#![no_main]

#[path = "../../tests/support/mock_transport.rs"]
mod mock_transport;
#[path = "../../tests/support/provider.rs"]
mod provider;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use libfuzzer_sys::fuzz_target;
use philo::{GenerateRequest, LlmClient, Message, ModelRef};

use mock_transport::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use provider::TestProvider;

fuzz_target!(|data: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let response = MockResponse::new(
            StatusCode::OK,
            headers,
            vec![MockBodyItem::chunk(Bytes::copy_from_slice(data))],
        );
        let transport = MockTransport::scripted([MockExchange::response(response)]);
        let provider = TestProvider::new(
            "https://test.invalid/v1/messages",
            "fuzz-placeholder-key",
        )
        .expect("static profile")
        .with_anthropic_messages()
        .build()
        .expect("runtime profile");
        let client = LlmClient::new(provider, transport);
        let request = GenerateRequest::new(
            ModelRef::new("test-only", "fuzz-model").expect("static model"),
            vec![Message::user("value-free fuzz fixture")],
        );
        let _ = client.complete(request).await;
    });
});
