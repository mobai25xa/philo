//! Official `OpenAI` Chat Completions protocol support.

// P1-011 deliberately lands before P1-016 wires the adapter into LlmClient.
#![allow(dead_code)]

mod request;
mod state;
mod tool_wire;
mod wire;

#[allow(unused_imports)]
pub(crate) use request::{EncodedOpenAiChatRequest, OpenAiChatRequestAdapter};
#[allow(unused_imports)]
pub(crate) use state::{
    OpenAiChatEventStream, OpenAiChatStreamContext, decode_openai_chat_stream,
    decode_openai_chat_stream_with_limits,
};
