use super::super::request::CapabilityStatus;

/// How missing tool results are handled before the next request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingToolResultPolicy {
    /// Reject the history until results are complete.
    Reject,
}

/// How unsupported content parts are handled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedContentPolicy {
    /// Reject the history immediately.
    Reject,
    /// Drop the part and record a diagnostic for compatible dialect input.
    DropWithDiagnostic,
}

/// How thinking/opaque reasoning may be replayed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingReplayPolicy {
    /// Keep opaque data only when source provider/model/protocol match.
    SameSourceOnly,
    /// Drop all thinking content and opaque signatures.
    DropAll,
}

/// Protocol-legality controls for history normalization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryPolicy {
    /// Missing tool-result policy.
    pub missing_tool_result: MissingToolResultPolicy,
    /// Unsupported content policy.
    pub unsupported_content: UnsupportedContentPolicy,
    /// Thinking replay policy.
    pub thinking_replay: ThinkingReplayPolicy,
}

impl HistoryPolicy {
    /// Official `OpenAI` history policy.
    pub const fn official_openai() -> Self {
        Self {
            missing_tool_result: MissingToolResultPolicy::Reject,
            unsupported_content: UnsupportedContentPolicy::Reject,
            thinking_replay: ThinkingReplayPolicy::DropAll,
        }
    }
}

/// Capability slice required by history normalization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryCapabilities {
    /// Developer-role support on the target profile.
    pub developer_role: CapabilityStatus,
    /// Vision/image support on the target profile.
    pub vision_input: CapabilityStatus,
}

impl HistoryCapabilities {
    /// Creates history capabilities from explicit three-state values.
    pub const fn new(developer_role: CapabilityStatus, vision_input: CapabilityStatus) -> Self {
        Self {
            developer_role,
            vision_input,
        }
    }

    /// Official defaults before an exact model override is applied.
    pub const fn official_openai_defaults() -> Self {
        Self {
            developer_role: CapabilityStatus::Supported,
            vision_input: CapabilityStatus::Unknown,
        }
    }
}

/// Where a dialect policy decision originated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicySource {
    /// Explicit request selection.
    Request,
    /// Exact model capability profile.
    ModelProfile,
    /// Provider profile defaults.
    ProviderProfile,
    /// Protocol default.
    ProtocolDefault,
}

/// Official tool-choice wire shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolChoiceWireFormat {
    /// Nested `OpenAI` function object.
    OpenAiNestedFunction,
    /// Anthropic Messages `tool` choice object.
    AnthropicTool,
}

/// Whether tool result messages include the tool name on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolResultNamePolicy {
    /// Omit the tool name field.
    Omit,
    /// Include the tool name when available.
    Include,
    /// Require the tool name field.
    Require,
}

/// How tool-call ids are encoded for the target protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCallIdPolicy {
    /// Preserve domain ids exactly.
    Preserve,
    /// Sanitize to official `OpenAI` historical id rules.
    OpenAi,
}

/// How thinking/reasoning is represented on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingWireFormat {
    /// Thinking is unsupported on this dialect.
    Unsupported,
    /// Official `OpenAI` `reasoning_effort` request path only.
    OpenAiReasoningEffort,
    /// Anthropic Messages adaptive-thinking object.
    AnthropicAdaptive,
}

/// How images are represented on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageWireFormat {
    /// Official `OpenAI` `image_url` content parts.
    OpenAiImageUrl,
    /// Anthropic Messages source objects.
    AnthropicSource,
}

/// Streaming usage request policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamUsagePolicy {
    /// Request `include_usage`.
    IncludeUsage,
    /// Streaming usage is unsupported.
    Unsupported,
    /// Anthropic Messages event-level usage snapshots.
    AnthropicSnapshots,
}

/// Structured-output wire format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuredOutputWireFormat {
    /// Official `OpenAI` `response_format` object.
    OpenAiResponseFormat,
    /// Anthropic Messages `output_config.format` object.
    AnthropicOutputConfig,
}

/// Complete dialect strategy group for one encoding target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DialectPolicy {
    /// Source of this complete policy group.
    pub source: PolicySource,
    /// Tool-choice encoding.
    pub tool_choice: ToolChoiceWireFormat,
    /// Tool-result name encoding.
    pub tool_result_name: ToolResultNamePolicy,
    /// Tool-call id encoding.
    pub tool_call_id: ToolCallIdPolicy,
    /// Thinking encoding.
    pub thinking: ThinkingWireFormat,
    /// Image encoding.
    pub image: ImageWireFormat,
    /// Streaming usage encoding.
    pub stream_usage: StreamUsagePolicy,
    /// Structured-output encoding.
    pub structured_output: StructuredOutputWireFormat,
}

impl DialectPolicy {
    /// Official `OpenAI` Chat Completions dialect policy.
    pub const fn official_openai() -> Self {
        Self {
            source: PolicySource::ProtocolDefault,
            tool_choice: ToolChoiceWireFormat::OpenAiNestedFunction,
            tool_result_name: ToolResultNamePolicy::Omit,
            tool_call_id: ToolCallIdPolicy::OpenAi,
            thinking: ThinkingWireFormat::OpenAiReasoningEffort,
            image: ImageWireFormat::OpenAiImageUrl,
            stream_usage: StreamUsagePolicy::IncludeUsage,
            structured_output: StructuredOutputWireFormat::OpenAiResponseFormat,
        }
    }

    /// Official Anthropic Messages dialect policy.
    pub const fn official_anthropic() -> Self {
        Self {
            source: PolicySource::ProtocolDefault,
            tool_choice: ToolChoiceWireFormat::AnthropicTool,
            tool_result_name: ToolResultNamePolicy::Omit,
            tool_call_id: ToolCallIdPolicy::Preserve,
            thinking: ThinkingWireFormat::AnthropicAdaptive,
            image: ImageWireFormat::AnthropicSource,
            stream_usage: StreamUsagePolicy::AnthropicSnapshots,
            structured_output: StructuredOutputWireFormat::AnthropicOutputConfig,
        }
    }
}
