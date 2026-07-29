#![no_main]

use libfuzzer_sys::fuzz_target;
use philo::domain::history::{
    DialectPolicy, HistoryCapabilities, HistoryPolicy, normalize_history,
};
use philo::domain::request::CapabilityStatus;
use philo::domain::schema::{SchemaLimits, ToolSchema};
use philo::domain::tools::ToolArguments;
use philo::{Message, MessageRole};

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data)
        && let Ok(schema) = ToolSchema::new(value.clone())
    {
        let _ = schema.validate_instance(&value, SchemaLimits::official());
    }

    let text = String::from_utf8_lossy(data);
    let _ = ToolArguments::from_raw_json(text.as_ref());

    let messages = data
        .chunks(64)
        .take(128)
        .enumerate()
        .map(|(index, chunk)| {
            let text = String::from_utf8_lossy(chunk);
            match index % 4 {
                0 => Message::user(text),
                1 => Message::assistant(text),
                2 => Message::system(text),
                _ => Message::new(MessageRole::Developer, vec![philo::ContentPart::text(text)]),
            }
        })
        .collect::<Vec<_>>();
    let capabilities =
        HistoryCapabilities::new(CapabilityStatus::Supported, CapabilityStatus::Unknown);
    let _ = normalize_history(
        &messages,
        &capabilities,
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    );
});
