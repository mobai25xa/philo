//! Protocol-private Anthropic Messages wire types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(super) struct MessagesRequestWire {
    pub(super) model: String,
    pub(super) max_tokens: u32,
    pub(super) messages: Vec<MessageWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) system: Option<Vec<SystemBlockWire>>,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<Vec<ToolWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<ToolChoiceWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thinking: Option<ThinkingConfigWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) output_config: Option<OutputConfigWire>,
}

#[derive(Serialize)]
pub(super) struct SystemBlockWire {
    #[serde(rename = "type")]
    kind: TextKindWire,
    text: String,
}

impl SystemBlockWire {
    pub(super) fn text(text: String) -> Self {
        Self {
            kind: TextKindWire::Text,
            text,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TextKindWire {
    Text,
}

#[derive(Serialize)]
pub(super) struct MessageWire {
    pub(super) role: MessageRoleWire,
    pub(super) content: Vec<RequestContentBlockWire>,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MessageRoleWire {
    User,
    Assistant,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum RequestContentBlockWire {
    Text {
        text: String,
    },
    Image {
        source: ImageSourceWire,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ImageSourceWire {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Serialize)]
pub(super) struct ToolWire {
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    pub(super) input_schema: Value,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) strict: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ToolChoiceWire {
    None {
        disable_parallel_tool_use: bool,
    },
    Auto {
        disable_parallel_tool_use: bool,
    },
    Any {
        disable_parallel_tool_use: bool,
    },
    Tool {
        name: String,
        disable_parallel_tool_use: bool,
    },
}

#[derive(Serialize)]
pub(super) struct OutputConfigWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) format: Option<OutputFormatWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) effort: Option<AnthropicEffortWire>,
}

#[derive(Serialize)]
pub(super) struct ThinkingConfigWire {
    #[serde(rename = "type")]
    pub(super) kind: ThinkingKindWire,
    pub(super) display: ThinkingDisplayWire,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ThinkingKindWire {
    Adaptive,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ThinkingDisplayWire {
    Omitted,
    Summarized,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AnthropicEffortWire {
    Low,
    Medium,
    High,
    Max,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum OutputFormatWire {
    JsonSchema { schema: Value },
}

#[derive(Deserialize)]
pub(super) struct MessageStartEventWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) message: MessageStartWire,
}

#[derive(Deserialize)]
pub(super) struct MessageStartWire {
    pub(super) id: String,
    pub(super) model: String,
    #[serde(default)]
    pub(super) usage: UsageWire,
}

#[derive(Deserialize)]
pub(super) struct ContentBlockStartEventWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) index: u32,
    pub(super) content_block: ContentBlockStartWire,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ContentBlockStartWire {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Thinking {
        thinking: String,
    },
    RedactedThinking {
        data: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub(super) struct ContentBlockDeltaEventWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) index: u32,
    pub(super) delta: ContentBlockDeltaWire,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ContentBlockDeltaWire {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub(super) struct IndexedEventWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) index: u32,
}

#[derive(Deserialize)]
pub(super) struct MessageDeltaEventWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) delta: MessageDeltaWire,
    #[serde(default)]
    pub(super) usage: UsageWire,
}

#[derive(Deserialize)]
pub(super) struct MessageDeltaWire {
    pub(super) stop_reason: Option<String>,
    #[allow(dead_code)]
    pub(super) stop_sequence: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub(super) struct UsageWire {
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) cache_creation_input_tokens: Option<u64>,
    pub(super) cache_read_input_tokens: Option<u64>,
    pub(super) thinking_tokens: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct TypeOnlyEventWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
}

#[derive(Deserialize)]
pub(super) struct ErrorEventWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) error: ErrorWire,
}

#[derive(Deserialize)]
pub(super) struct ErrorWire {
    #[serde(rename = "type")]
    pub(super) kind: String,
    #[allow(dead_code)]
    pub(super) message: String,
}
