//! Stable consumer for tool schemas and application-owned execution history.

use philo::domain::ids::ToolName;
use philo::domain::schema::ToolSchema;
use philo::domain::tools::ToolDefinition;
use philo::LlmError;

/// Builds one bounded tool definition through public owner modules.
pub fn tool() -> Result<ToolDefinition, LlmError> {
    Ok(ToolDefinition::new(
        ToolName::new("lookup")?,
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"],
            "additionalProperties": false
        }))?,
    ))
}
