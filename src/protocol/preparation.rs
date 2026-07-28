//! Value-free preparation primitives shared by concrete protocol drivers.

use http::{HeaderValue, header};

use super::{HeaderOperation, MaxOutputTokensSource, RequestFacts, ResponseFormatKind};
use crate::domain::{ContentPart, MessageRole, ThinkingRequest};
use crate::plan::PlannedRequest;

/// Request facts whose meaning is independent of the selected wire protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommonRequestFacts {
    pub(super) contains_tools: bool,
    pub(super) contains_images: bool,
    pub(super) response_format: ResponseFormatKind,
    reasoning: RequestFieldPresence,
    max_output_tokens: RequestFieldPresence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestFieldPresence {
    Explicit,
    Omitted,
}

impl CommonRequestFacts {
    pub(super) fn scan(request: &PlannedRequest) -> Self {
        let contains_tools = !request.options.tools().is_empty()
            || request.messages.iter().any(|message| {
                message.role() == MessageRole::Tool
                    || message
                        .content()
                        .iter()
                        .any(|part| matches!(part, ContentPart::ToolCall(_)))
            });
        let contains_images = request.messages.iter().any(|message| {
            message
                .content()
                .iter()
                .any(|part| matches!(part, ContentPart::Image(_)))
        });

        Self {
            contains_tools,
            contains_images,
            response_format: ResponseFormatKind::from(request.options.response_format()),
            reasoning: if matches!(
                request.options.reasoning(),
                ThinkingRequest::ProviderDefault
            ) {
                RequestFieldPresence::Omitted
            } else {
                RequestFieldPresence::Explicit
            },
            max_output_tokens: if request.options.max_output_tokens().is_some() {
                RequestFieldPresence::Explicit
            } else {
                RequestFieldPresence::Omitted
            },
        }
    }

    pub(super) const fn reasoning_requested(self) -> bool {
        matches!(self.reasoning, RequestFieldPresence::Explicit)
    }

    pub(super) const fn max_output_tokens_requested(self) -> bool {
        matches!(self.max_output_tokens, RequestFieldPresence::Explicit)
    }
}

/// Decisions whose meaning remains owned by one concrete protocol driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProtocolFactDecisions {
    pub(super) reasoning_enabled: bool,
    pub(super) max_output_tokens_source: MaxOutputTokensSource,
}

pub(super) fn request_facts(
    common: CommonRequestFacts,
    decisions: ProtocolFactDecisions,
) -> RequestFacts {
    RequestFacts::from_common(common, decisions)
}

/// Protocol header intents shared by JSON request / SSE response operations.
pub(super) fn standard_json_sse_header_operations() -> Vec<HeaderOperation> {
    vec![
        HeaderOperation::set(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        ),
        HeaderOperation::set(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::standard_json_sse_header_operations;

    #[test]
    fn standard_headers_are_owned_intents_without_request_values() {
        let operations = standard_json_sse_header_operations();
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0].name(), http::header::CONTENT_TYPE);
        assert_eq!(operations[1].name(), http::header::ACCEPT);
    }
}
