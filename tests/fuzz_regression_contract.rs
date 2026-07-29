//! Promoted fuzz corpus is replayed by the ordinary offline test suite.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use philo::domain::history::{
    DialectPolicy, HistoryCapabilities, HistoryPolicy, normalize_history,
};
use philo::domain::request::CapabilityStatus;
use philo::domain::schema::{SchemaLimits, ToolSchema};
use philo::domain::tools::ToolArguments;
use philo::error::BodySummary;
use philo::protocol_options::{AnthropicRawExtension, OpenAiChatRawExtension};
use philo::provider::endpoint::EndpointConfig;
use philo::provider::headers::HeaderOperation;
use philo::transport::{ByteStream, SseConfig, SseDecoder};
use philo::{ContentPart, GenerateRequest, LlmClient, Message, MessageRole, ModelRef};
use philo_config::ProviderConfigDocument;
use support::mock_transport::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use support::provider::TestProvider;

fn corpus(target: &str) -> Vec<(PathBuf, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz")
        .join("corpus")
        .join(target);
    let mut cases = fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", root.display()))
        .map(|entry| entry.expect("corpus entry").path())
        .filter(|path| path.is_file())
        .map(|path| {
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            (path, bytes)
        })
        .collect::<Vec<_>>();
    cases.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(!cases.is_empty(), "{target} corpus must not be empty");
    cases
}

#[tokio::test]
async fn sse_decoder_corpus_never_panics_or_stalls() {
    for (path, data) in corpus("sse_decoder") {
        let upstream: ByteStream = Box::pin(stream::once(async move { Ok(Bytes::from(data)) }));
        let config = SseConfig::new(64 * 1024, 16 * 1024).unwrap();
        let results = SseDecoder::with_config(upstream, config)
            .collect::<Vec<_>>()
            .await;
        assert!(
            results.len() <= 4096,
            "unbounded event count for {}",
            path.display()
        );
    }
}

async fn replay_protocol(target: &str, anthropic: bool) {
    for (path, data) in corpus(target) {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let response = MockResponse::new(
            StatusCode::OK,
            headers,
            vec![MockBodyItem::chunk(Bytes::from(data))],
        );
        let transport = MockTransport::scripted([MockExchange::response(response)]);
        let profile = TestProvider::new(
            "https://test.invalid/v1/generate",
            "regression-placeholder-key",
        )
        .unwrap();
        let profile = if anthropic {
            profile.with_anthropic_messages()
        } else {
            profile
        };
        let client = LlmClient::new(profile.build().unwrap(), transport);
        let request = GenerateRequest::new(
            ModelRef::new("test-only", "fuzz-model").unwrap(),
            vec![Message::user("value-free regression fixture")],
        );
        let result = client.complete(request).await;
        let diagnostic = format!("{result:?}");
        assert!(
            !diagnostic.contains("regression-placeholder-key"),
            "secret leaked while replaying {}",
            path.display()
        );
    }
}

#[tokio::test]
async fn protocol_state_machine_corpora_are_bounded_and_value_free() {
    replay_protocol("openai_stream", false).await;
    replay_protocol("anthropic_stream", true).await;
}

#[test]
fn endpoint_and_header_corpus_is_rejected_or_typed() {
    for (_, data) in corpus("endpoint_and_headers") {
        let text = String::from_utf8_lossy(&data);
        let _ = EndpointConfig::absolute(&text);
        let split = data.len() / 2;
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(&data[..split]),
            HeaderValue::from_bytes(&data[split..]),
        ) {
            let _ = HeaderOperation::set(name, value);
        }
    }
}

#[test]
fn domain_schema_history_and_tool_corpus_is_bounded() {
    for (_, data) in corpus("domain_schema_history_tools") {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data)
            && let Ok(schema) = ToolSchema::new(value.clone())
        {
            let _ = schema.validate_instance(&value, SchemaLimits::official());
        }
        let text = String::from_utf8_lossy(&data);
        let _ = ToolArguments::from_raw_json(text.as_ref());
        let messages = data
            .chunks(64)
            .take(128)
            .enumerate()
            .map(|(index, chunk)| {
                let text = String::from_utf8_lossy(chunk);
                match index % 4 {
                    0 => Message::user(text),
                    1 => Message::assistant(text),
                    2 => Message::system(text),
                    _ => Message::new(MessageRole::Developer, vec![ContentPart::text(text)]),
                }
            })
            .collect::<Vec<_>>();
        let capabilities =
            HistoryCapabilities::new(CapabilityStatus::Supported, CapabilityStatus::Unknown);
        let _ = normalize_history(
            &messages,
            &capabilities,
            &DialectPolicy::official_openai(),
            &HistoryPolicy::official_openai(),
        );
    }
}

#[test]
fn raw_body_error_and_config_corpora_are_value_free_and_roundtrip_safe() {
    for (_, data) in corpus("raw_body_and_error") {
        let summary = BodySummary::from_bytes(&data, 4096);
        let _ = format!("{summary:?}");
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data) {
            let openai = OpenAiChatRawExtension::dangerous_from_object(value.clone());
            let anthropic = AnthropicRawExtension::dangerous_from_object(value);
            let _ = format!("{openai:?}{anthropic:?}");
        }
    }

    for (_, data) in corpus("config_parser") {
        let input = String::from_utf8_lossy(&data);
        if let Ok(document) = ProviderConfigDocument::from_json(&input) {
            let current = document.to_current_json().unwrap();
            ProviderConfigDocument::from_json(&current).unwrap();
        }
    }
}
