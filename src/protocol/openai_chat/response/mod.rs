//! Private `OpenAI` Chat response decoding and state management.

mod machine;
mod stream;
mod terminal;
mod tool_calls;
mod usage;

pub(crate) use stream::{
    OpenAiChatStreamContext, decode_openai_chat_stream_with_plan,
    decode_openai_chat_stream_with_policy,
};

use crate::error::{ErrorStage, LlmError, ProtocolError};

pub(super) fn protocol(message: impl Into<String>) -> LlmError {
    ProtocolError::at_stage(ErrorStage::Protocol, message).into()
}
