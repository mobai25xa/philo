//! Declares aggregation-gateway routing through the bounded body extension axis.
//!
//! Gateway routing preferences are one provider's product parameters, not SDK
//! concepts. They travel as unknown top-level fields of the `OpenAI` Chat request
//! body, which is exactly what the bounded raw extension admits.
//!
//! The `dangerous` name is not decoration: these fields are **not portable**. A body
//! written for one gateway means nothing on another, and the SDK gives their contents
//! no compatibility guarantee. Core request fields, headers, credentials, and protocol
//! versions stay protected — the extension cannot reach them.

use std::error::Error;

use philo::domain::request::GenerationOptions;
use philo::protocol_options::{OpenAiChatOptions, OpenAiChatRawExtension};
use philo::{GenerateRequest, Message, ModelRef, ProtocolOptions};
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let routing = OpenAiChatRawExtension::dangerous_from_object(json!({
        "provider": {
            "only": ["alpha", "beta"],
            "order": ["beta", "alpha"],
            "allow_fallbacks": false,
            "data_collection": "deny",
            "sort": "latency"
        }
    }))?;

    for diagnostic in OpenAiChatOptions::new()
        .with_raw_extension(routing.clone())
        .diagnostics()
    {
        println!("diagnostic={diagnostic:?}");
    }

    let request = GenerateRequest::new(
        ModelRef::new("openrouter", "some-model")?,
        vec![Message::user("hello")],
    )
    .with_options(
        GenerationOptions::new()
            .with_protocol_options(OpenAiChatOptions::new().with_raw_extension(routing)),
    );

    println!(
        "protocol_options={}",
        request
            .options()
            .protocol_options()
            .map_or("none", ProtocolOptions::protocol_id)
    );

    // The axis is bounded: an SDK-owned field is refused before any request exists.
    let refused = OpenAiChatRawExtension::dangerous_from_object(json!({"messages": []}));
    println!("sdk_owned_field_refused={}", refused.is_err());
    Ok(())
}
