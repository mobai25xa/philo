//! Private response compatibility hooks.

use crate::error::LlmError;
use crate::provider::ToolArgumentsCompat;

use super::super::wire::ToolArgumentsWire;

pub(in crate::protocol::openai_chat) fn normalize_tool_arguments(
    value: &ToolArgumentsWire,
    compat: ToolArgumentsCompat,
) -> Result<String, LlmError> {
    match value {
        ToolArgumentsWire::String(value) => Ok(value.clone()),
        ToolArgumentsWire::Object(value)
            if matches!(compat, ToolArgumentsCompat::StringOrObject) =>
        {
            serde_json::to_string(value).map_err(|_| {
                super::super::response::protocol("failed to normalize tool argument object")
            })
        }
        ToolArgumentsWire::Object(_) => Err(super::super::response::protocol(
            "tool arguments must be a JSON string for this profile",
        )),
    }
}
