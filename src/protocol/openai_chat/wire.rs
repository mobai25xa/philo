use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::MessageRole;

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
}

impl<'a> ChatCompletionRequestWire<'a> {
    pub(super) fn new(
        model: &'a str,
        messages: Vec<MessageWire<'a>>,
        temperature: Option<f64>,
        max_completion_tokens: Option<u32>,
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
        }
    }
}

#[derive(Serialize)]
pub(super) struct MessageWire<'a> {
    role: MessageRoleWire,
    content: &'a str,
}

impl<'a> MessageWire<'a> {
    pub(super) fn new(role: MessageRole, content: &'a str) -> Self {
        Self {
            role: role.into(),
            content,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum MessageRoleWire {
    Developer,
    System,
    User,
    Assistant,
}

impl From<MessageRole> for MessageRoleWire {
    fn from(value: MessageRole) -> Self {
        match value {
            MessageRole::Developer => Self::Developer,
            MessageRole::System => Self::System,
            MessageRole::User => Self::User,
            MessageRole::Assistant => Self::Assistant,
        }
    }
}

#[derive(Serialize)]
struct StreamOptionsWire {
    include_usage: bool,
}

#[derive(Deserialize)]
pub(super) struct ChatCompletionChunkWire {
    pub(super) id: Option<String>,
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) choices: Vec<ChoiceWire>,
    pub(super) usage: Option<UsageWire>,
    pub(super) error: Option<serde_json::Value>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub(super) struct ChoiceWire {
    pub(super) index: i64,
    pub(super) delta: Option<DeltaWire>,
    pub(super) finish_reason: Option<String>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub(super) struct DeltaWire {
    pub(super) role: Option<String>,
    pub(super) content: Option<String>,
    pub(super) tool_calls: Option<serde_json::Value>,
    pub(super) function_call: Option<serde_json::Value>,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
pub(super) struct UsageWire {
    pub(super) prompt_tokens: i64,
    pub(super) completion_tokens: i64,
    pub(super) total_tokens: i64,
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, serde_json::Value>,
}
