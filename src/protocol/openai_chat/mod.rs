//! Official `OpenAI` Chat Completions protocol support.

mod driver;
mod request;
mod state;
mod structured_wire;
mod tool_wire;
mod wire;

pub(crate) use driver::OpenAiChatDriver;
pub(crate) use state::{OpenAiChatStreamContext, decode_openai_chat_stream_with_plan};
