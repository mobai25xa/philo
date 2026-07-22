//! Provider-independent domain types.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod content;
pub mod event;
pub mod history;
pub mod ids;
pub mod limits;
pub mod message;
pub mod request;
pub mod schema;
pub mod structured;
pub mod tools;
pub mod usage;

pub use content::{
    ContentPart, ImageContent, ImageDetail, ImageMime, ImageSource, OpaqueReasoning,
    RefusalContent, SourceIdentity, ThinkingContent,
};
pub use event::{
    AssistantEvent, AssistantMessage, FinishReason, Usage, collect_assistant_message,
    collect_assistant_message_for_format,
};
pub use history::{
    DiagnosticCode, DialectPolicy, HistoryCapabilities, HistoryPolicy, IdMapping, ImageWireFormat,
    MissingToolResultPolicy, NormalizationDiagnostic, NormalizedContext, PolicySource,
    StreamUsagePolicy, StructuredOutputWireFormat, ThinkingReplayPolicy, ThinkingWireFormat,
    ToolCallIdPolicy, ToolChoiceWireFormat, ToolResultNamePolicy, UnsupportedContentPolicy,
    apply_thinking_replay_policy, drop_opaque_reasoning, normalize_history,
};
pub use ids::{
    ContentIndex, GenerationId, LocalRequestId, ModelId, ModelRef, ProtocolId, ProviderId,
    ProviderRequestId, ToolCallId, ToolName, TraceId, WireToolIndex,
};
pub use limits::{ResourceLimits, ResourceLimitsBuilder};
pub use message::{Message, MessageRole, ToolResultMessage};
pub use request::{
    CapabilitySet, CapabilityStatus, GenerateRequest, GenerationOptions, LlmRequest,
    ReasoningEffort, ReasoningEffortSupport, RequestMetadata, RequestTimeout, ThinkingRequest,
};
pub(crate) use request::{
    RequestValidationLimits, validate_planned_request, validate_request_shape,
};
pub use schema::{SchemaLimits, ToolSchema};
pub use structured::{ResponseFormat, StructuredSchema};
pub use tools::{
    ParallelToolCalls, ToolArguments, ToolCall, ToolChoice, ToolDefinition, ToolLimits,
    ValidatedToolCall, validate_tool_call, validate_tool_options,
};
pub use usage::{
    CostEstimate, CurrencyCode, MoneyAmount, PriceProfile, TokenCount, UsageDetails,
    UsageMergeOutcome, estimate_cost, merge_usage_details,
};
