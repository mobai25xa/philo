use serde::Deserialize;

use crate::domain::ProviderRequestId;
use crate::error::BodySummary;
use crate::transport::LimitedBody;

const MAX_PROVIDER_CODE_BYTES: usize = 128;

pub(crate) struct AnthropicHttpErrorDetails {
    pub(crate) summary: BodySummary,
    pub(crate) provider_code: Option<String>,
    pub(crate) request_id: Option<ProviderRequestId>,
}

#[derive(Deserialize)]
struct ErrorEnvelopeWire {
    error: ErrorBodyWire,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct ErrorBodyWire {
    #[serde(rename = "type")]
    kind: String,
}

pub(crate) fn decode_http_error(body: &LimitedBody) -> AnthropicHttpErrorDetails {
    let parsed = serde_json::from_slice::<ErrorEnvelopeWire>(body.bytes()).ok();
    let provider_code = parsed
        .as_ref()
        .and_then(|wire| bounded_provider_code(&wire.error.kind));
    let request_id = parsed
        .and_then(|wire| wire.request_id)
        .and_then(|value| ProviderRequestId::new(value).ok());
    let label = provider_code.as_ref().map_or_else(
        || "Anthropic HTTP error".to_owned(),
        |code| format!("Anthropic HTTP error ({code})"),
    );
    AnthropicHttpErrorDetails {
        summary: BodySummary::from_bytes(label.as_bytes(), label.len()),
        provider_code,
        request_id,
    }
}

fn bounded_provider_code(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= MAX_PROVIDER_CODE_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::decode_http_error;
    use crate::transport::LimitedBody;

    #[test]
    fn typed_error_keeps_only_code_and_request_id() {
        let body = LimitedBody::from_test_parts(
            Bytes::from_static(
                br#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt-canary"},"request_id":"req_test"}"#,
            ),
            false,
        );
        let details = decode_http_error(&body);
        assert_eq!(
            details.provider_code.as_deref(),
            Some("invalid_request_error")
        );
        assert_eq!(details.request_id.unwrap().as_str(), "req_test");
        assert!(!details.summary.as_str().contains("prompt-canary"));
    }
}
