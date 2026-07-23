use super::super::wire::UsageWire;
use super::protocol;
use crate::domain::{TokenCount, Usage, UsageDetails};
use crate::error::LlmError;

pub(super) fn parse_usage_details(wire: &UsageWire) -> Result<Option<UsageDetails>, LlmError> {
    let input = optional_token_count(wire.prompt_tokens, "usage.prompt_tokens")?;
    let output = optional_token_count(wire.completion_tokens, "usage.completion_tokens")?;
    let total = optional_token_count(wire.total_tokens, "usage.total_tokens")?;
    let cached_input = optional_token_count(
        wire.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
        "usage.prompt_tokens_details.cached_tokens",
    )?;
    let cache_write = optional_token_count(
        wire.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens),
        "usage.prompt_tokens_details.cache_write_tokens",
    )?;
    let reasoning = optional_token_count(
        wire.completion_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens),
        "usage.completion_tokens_details.reasoning_tokens",
    )?;

    let details = UsageDetails::new(input, output, total, cached_input, cache_write, reasoning);
    details
        .validate_relationships()
        .map_err(|error| protocol(error.message()))?;
    if !details.has_any_known() {
        return Ok(None);
    }
    Ok(Some(details))
}

fn optional_token_count(value: Option<i64>, field: &str) -> Result<TokenCount, LlmError> {
    match value {
        None => Ok(TokenCount::Unknown),
        Some(raw) => {
            let count = u64::try_from(raw)
                .map_err(|_| protocol(format!("{field} must be non-negative")))?;
            Ok(TokenCount::Known(count))
        }
    }
}

pub(super) fn core_usage_from_details(details: UsageDetails) -> Result<Usage, &'static str> {
    match (
        details.input_tokens(),
        details.output_tokens(),
        details.total_tokens(),
    ) {
        (TokenCount::Known(input), TokenCount::Known(output), TokenCount::Known(total)) => {
            Usage::new(input, output, total)
                .map_err(|_| "usage total does not equal input + output")
        }
        _ => Err("core usage counters are incomplete"),
    }
}
