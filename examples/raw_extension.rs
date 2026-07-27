//! Migrates non-portable `OpenRouter` routing parameters to the bounded body axis.

use std::error::Error;

use philo::protocol_options::{OpenAiChatOptions, OpenAiChatRawExtension};
use philo::{GenerateRequest, GenerationOptions, Message, ModelRef};
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let routing = OpenAiChatRawExtension::dangerous_from_object(json!({
        "provider": {
            "only": ["alpha", "beta"],
            "allow_fallbacks": false,
            "data_collection": "deny"
        }
    }))?;
    let request = GenerateRequest::new(
        ModelRef::new("openrouter", "example/model")?,
        vec![Message::user("hello")],
    )
    .with_options(
        GenerationOptions::new()
            .with_protocol_options(OpenAiChatOptions::new().with_raw_extension(routing)),
    );

    assert!(request.options().protocol_options().is_some());
    assert!(OpenAiChatRawExtension::dangerous_from_object(json!({"messages": []})).is_err());
    Ok(())
}
