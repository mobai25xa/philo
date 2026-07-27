//! Owned values exchanged by planning and execution layers.
#![allow(dead_code)]

use std::fmt;

use http::HeaderMap;

use super::CallPolicySnapshot;
use crate::domain::{
    GenerationOptions, IdMapping, Message, ModelRef, NormalizationDiagnostic, PolicySource,
    RequestTimeout, SourceIdentity,
};

/// Fully resolved, immutable plan for one logical generation call.
#[derive(Clone)]
pub(crate) struct ResolvedCallPlan {
    pub(crate) planned: PlannedRequest,
    pub(crate) policy: CallPolicySnapshot,
    pub(crate) execution: CallExecutionIntent,
    pub(crate) provenance: PlanProvenance,
}

impl fmt::Debug for ResolvedCallPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedCallPlan")
            .field("planned", &self.planned)
            .field("policy", &self.policy)
            .field("execution", &self.execution)
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// Provider-independent request after history normalization and validation.
#[derive(Clone)]
pub(crate) struct PlannedRequest {
    pub(crate) model: ModelRef,
    pub(crate) source: SourceIdentity,
    pub(crate) messages: Vec<Message>,
    pub(crate) options: GenerationOptions,
    pub(crate) normalization: NormalizationReport,
}

impl fmt::Debug for PlannedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlannedRequest")
            .field("model", &self.model)
            .field("message_count", &self.messages.len())
            .field("tool_count", &self.options.tools().len())
            .field("has_request_headers", &(!self.options.headers().is_empty()))
            .field("normalization", &self.normalization)
            .finish_non_exhaustive()
    }
}

/// Value-free summary of one history normalization pass.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct NormalizationReport {
    pub(crate) mappings: Vec<IdMapping>,
    pub(crate) diagnostics: Vec<NormalizationDiagnostic>,
    pub(crate) input_message_count: usize,
    pub(crate) output_message_count: usize,
}

impl fmt::Debug for NormalizationReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizationReport")
            .field("mapping_count", &self.mappings.len())
            .field("diagnostics", &self.diagnostics)
            .field("input_message_count", &self.input_message_count)
            .field("output_message_count", &self.output_message_count)
            .finish()
    }
}

/// Sources used to resolve policy and model decisions for a call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlanProvenance {
    pub(crate) capability_source: PolicySource,
    pub(crate) compat_source: PolicySource,
    pub(crate) model_override_applied: bool,
}

/// Non-protocol execution inputs captured before the first await point.
#[derive(Clone)]
pub(crate) struct CallExecutionIntent {
    pub(crate) request_headers: HeaderMap,
    pub(crate) timeout: Option<RequestTimeout>,
}

impl fmt::Debug for CallExecutionIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut header_names = self
            .request_headers
            .keys()
            .map(http::HeaderName::as_str)
            .collect::<Vec<_>>();
        header_names.sort_unstable();
        let timeout_kind = match self.timeout {
            Some(RequestTimeout::After(_)) => Some("after"),
            Some(RequestTimeout::At(_)) => Some("at"),
            None => None,
        };
        formatter
            .debug_struct("CallExecutionIntent")
            .field("header_names", &header_names)
            .field("timeout_kind", &timeout_kind)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue, header};

    use super::{CallExecutionIntent, NormalizationReport, PlannedRequest};
    use crate::domain::{GenerateRequest, GenerationOptions, Message, ModelRef};

    const CANARY: &str = "contract-canary-secret";

    #[test]
    fn contract_debug_does_not_expose_messages_or_header_values() {
        let options = GenerationOptions::new().with_header(
            header::HeaderName::from_static("x-private-value"),
            HeaderValue::from_static(CANARY),
        );
        let request = GenerateRequest::new(
            ModelRef::new("official-openai", "gpt-test").unwrap(),
            vec![Message::user(CANARY)],
        )
        .with_options(options.clone());
        let planned = PlannedRequest {
            model: request.model().clone(),
            source: crate::domain::SourceIdentity::new(
                request.model().provider().clone(),
                request.model().model().clone(),
                crate::domain::ProtocolId::new("openai-chat").unwrap(),
            ),
            messages: request.messages().to_vec(),
            options,
            normalization: NormalizationReport {
                mappings: Vec::new(),
                diagnostics: Vec::new(),
                input_message_count: 1,
                output_message_count: 1,
            },
        };
        assert!(!format!("{planned:?}").contains(CANARY));

        let mut request_headers = HeaderMap::new();
        request_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer contract-canary-secret"),
        );
        let execution = CallExecutionIntent {
            request_headers,
            timeout: None,
        };
        let debug = format!("{execution:?}");
        assert!(debug.contains("authorization"));
        assert!(!debug.contains(CANARY));
        assert!(!debug.contains("Bearer"));
    }
}
