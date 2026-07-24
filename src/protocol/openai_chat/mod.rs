//! Official `OpenAI` Chat Completions protocol support.

mod compat;
mod driver;
mod request;
mod response;
mod structured_wire;
mod tool_wire;
mod wire;

pub(crate) use driver::OpenAiChatDriver;
#[allow(unused_imports)]
pub(crate) use response::{
    OpenAiChatStreamContext, decode_openai_chat_stream_with_plan,
    decode_openai_chat_stream_with_policy,
};
