//! Private provider error compatibility hooks.

use crate::error::LlmError;
use crate::provider::InlineErrorCompat;

pub(in crate::protocol::openai_chat) fn validate_inline_error(
    present: bool,
    policy: InlineErrorCompat,
) -> Result<(), LlmError> {
    if !present {
        return Ok(());
    }
    match policy {
        InlineErrorCompat::Reject => Err(super::super::response::protocol(
            "provider returned a JSON error object",
        )),
    }
}
