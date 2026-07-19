//! Provider-independent domain types.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod content;
pub mod event;
pub mod ids;
pub mod limits;
pub mod message;
pub mod request;
pub mod schema;
pub mod tools;

pub use content::{
    ContentPart, ImageContent, ImageDetail, ImageMime, ImageSource, OpaqueReasoning,
    RefusalContent, SourceIdentity, ThinkingContent,
};
pub use event::{AssistantEvent, AssistantMessage, FinishReason, Usage, collect_assistant_message};
pub use ids::{
    ContentIndex, GenerationId, LocalRequestId, ModelId, ModelRef, ProtocolId, ProviderId,
    ProviderRequestId, ToolCallId, ToolName, TraceId, WireToolIndex,
};
pub use limits::ResourceLimits;
pub use message::{Message, MessageRole};
pub use request::{
    CapabilitySet, CapabilityStatus, GenerateRequest, GenerationOptions, LlmRequest,
    ReasoningEffort, ReasoningEffortSupport, RequestMetadata, RequestTimeout,
};
pub use schema::{SchemaLimits, ToolSchema};
pub use tools::{
    ParallelToolCalls, ToolArguments, ToolCall, ToolChoice, ToolDefinition, ToolLimits,
    validate_tool_options,
};
