use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{ImageDetail, MessageRole};

use super::structured_wire::ResponseFormatWire;
use super::tool_wire::{ToolChoiceWire, ToolWire};

#[derive(Serialize)]
pub(super) struct ChatCompletionRequestWire<'a> {
    model: &'a str,
    messages: Vec<MessageWire<'a>>,
    stream: bool,
    stream_options: StreamOptionsWire,
    n: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolWire<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoiceWire<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormatWire<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffortWire>,
}

impl<'a> ChatCompletionRequestWire<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        model: &'a str,
        messages: Vec<MessageWire<'a>>,
        temperature: Option<f64>,
        max_completion_tokens: Option<u32>,
        tools: Option<Vec<ToolWire<'a>>>,
        tool_choice: Option<ToolChoiceWire<'a>>,
        parallel_tool_calls: Option<bool>,
        response_format: Option<ResponseFormatWire<'a>>,
        reasoning_effort: Option<ReasoningEffortWire>,
    ) -> Self {
        Self {
            model,
            messages,
            stream: true,
            stream_options: StreamOptionsWire {
                include_usage: true,
            },
            n: 1,
            temperature,
            max_completion_tokens,
            tools,
            tool_choice,
            parallel_tool_calls,
            response_format,
            reasoning_effort,
        }
    }
}

#[derive(Serialize)]
pub(super) struct MessageWire<'a> {
    role: MessageRoleWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<MessageContentWire<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<AssistantToolCallWire<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

impl<'a> MessageWire<'a> {
    pub(super) fn text(role: MessageRole, content: &'a str) -> Self {
        Self {
            role: role.into(),
            content: Some(MessageContentWire::Text(content)),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub(super) fn parts(role: MessageRole, parts: Vec<MessageContentPartWire<'a>>) -> Self {
        Self {
            role: role.into(),
            content: Some(MessageContentWire::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub(super) fn assistant(
        content: Option<MessageContentWire<'a>>,
        tool_calls: Option<Vec<AssistantToolCallWire<'a>>>,
    ) -> Self {
        Self {
            role: MessageRoleWire::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
        }
    }

    pub(super) fn tool_result(tool_call_id: &'a str, content: &'a str) -> Self {
        Self {
            role: MessageRoleWire::Tool,
            content: Some(MessageContentWire::Text(content)),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum MessageContentWire<'a> {
    Text(&'a str),
    OwnedText(String),
    Parts(Vec<MessageContentPartWire<'a>>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum MessageContentPartWire<'a> {
    Text { text: &'a str },
    ImageUrl { image_url: ImageUrlWire<'a> },
}

#[derive(Serialize)]
pub(super) struct ImageUrlWire<'a> {
    url: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<ImageDetailWire>,
}

impl<'a> ImageUrlWire<'a> {
    pub(super) fn new(url: Cow<'a, str>, detail: ImageDetail) -> Self {
        Self {
            url,
            detail: match detail {
                ImageDetail::Auto => None,
                ImageDetail::Low => Some(ImageDetailWire::Low),
                ImageDetail::High => Some(ImageDetailWire::High),
                ImageDetail::Original => Some(ImageDetailWire::Original),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ImageDetailWire {
    Low,
    High,
    Original,
}

#[derive(Serialize)]
pub(super) struct AssistantToolCallWire<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: FunctionKindWire,
    function: AssistantFunctionCallWire<'a>,
}

impl<'a> AssistantToolCallWire<'a> {
    pub(super) fn new(id: &'a str, name: &'a str, arguments: &'a str) -> Self {
        Self {
            id,
            kind: FunctionKindWire::Function,
            function: AssistantFunctionCallWire { name, arguments },
        }
    }
}

#[derive(Serialize)]
struct AssistantFunctionCallWire<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Clone, Copy, Serialize)]
enum FunctionKindWire {
    #[serde(rename = "function")]
    Function,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum MessageRoleWire {
    Developer,
    System,
    User,
    Assistant,
    Tool,
}

impl From<MessageRole> for MessageRoleWire {
    fn from(value: MessageRole) -> Self {
        match value {
            MessageRole::Developer => Self::Developer,
            MessageRole::System => Self::System,
            MessageRole::User => Self::User,
            MessageRole::Assistant => Self::Assistant,
            MessageRole::Tool => Self::Tool,
        }
    }
}

#[derive(Serialize)]
struct StreamOptionsWire {
    include_usage: bool,
}

#[derive(Clone, Copy, Serialize)]
pub(super) enum ReasoningEffortWire {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "minimal")]
    Minimal,
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    #[serde(rename = "max")]
    Max,
}

#[derive(Deserialize)]
pub(super) struct ChatCompletionChunkWire {
    pub(super) id: Option<String>,
    pub(super) object: Option<String>,
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) choices: Vec<ChoiceWire>,
    pub(super) usage: Option<UsageWire>,
    pub(super) error: Option<Value>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct ChoiceWire {
    pub(super) index: i64,
    pub(super) delta: Option<DeltaWire>,
    pub(super) finish_reason: Option<String>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct DeltaWire {
    pub(super) role: Option<String>,
    pub(super) content: Option<String>,
    pub(super) refusal: Option<String>,
    pub(super) tool_calls: Option<Vec<ToolCallDeltaWire>>,
    pub(super) function_call: Option<Value>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct ToolCallDeltaWire {
    pub(super) index: i64,
    pub(super) id: Option<String>,
    #[serde(rename = "type")]
    pub(super) kind: Option<String>,
    pub(super) function: Option<FunctionDeltaWire>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct FunctionDeltaWire {
    pub(super) name: Option<String>,
    pub(super) arguments: Option<String>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
pub(super) struct UsageWire {
    pub(super) prompt_tokens: Option<i64>,
    pub(super) completion_tokens: Option<i64>,
    pub(super) total_tokens: Option<i64>,
    pub(super) prompt_tokens_details: Option<PromptTokensDetailsWire>,
    pub(super) completion_tokens_details: Option<CompletionTokensDetailsWire>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct PromptTokensDetailsWire {
    pub(super) cached_tokens: Option<i64>,
    pub(super) cache_write_tokens: Option<i64>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct CompletionTokensDetailsWire {
    pub(super) reasoning_tokens: Option<i64>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, Value>,
}
