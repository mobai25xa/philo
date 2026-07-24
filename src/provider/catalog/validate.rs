//! Catalog validation helpers.

use crate::error::LlmError;

use super::entry::ModelEntry;

/// Validates a complete catalog entry and returns a value-free error.
///
/// # Errors
///
/// Returns a configuration error when the entry violates catalog invariants.
pub fn validate_entry(entry: &ModelEntry) -> Result<(), LlmError> {
    entry.validate()
}
