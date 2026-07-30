#![no_main]

use libfuzzer_sys::fuzz_target;
use philo::error::BodySummary;
use philo::protocol_options::{AnthropicRawExtension, OpenAiChatRawExtension};

fuzz_target!(|data: &[u8]| {
    let limit = data.first().map_or(0, |byte| usize::from(*byte) * 16);
    let summary = BodySummary::from_bytes(data, limit);
    let _ = format!("{summary:?}");
    let _ = summary.as_str();

    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let openai = OpenAiChatRawExtension::dangerous_from_object(value.clone());
        let anthropic = AnthropicRawExtension::dangerous_from_object(value);
        let _ = format!("{openai:?}{anthropic:?}");
    }
});
