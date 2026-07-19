use serde::Serialize;
use serde_json::Value;

use crate::domain::{ResponseFormat, StructuredSchema};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ResponseFormatWire<'a> {
    JsonObject,
    JsonSchema { json_schema: JsonSchemaWire<'a> },
}

#[derive(Serialize)]
pub(super) struct JsonSchemaWire<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    schema: &'a Value,
    strict: bool,
}

impl<'a> ResponseFormatWire<'a> {
    pub(super) fn from_domain(format: &'a ResponseFormat) -> Option<Self> {
        match format {
            ResponseFormat::Text => None,
            ResponseFormat::JsonObject => Some(Self::JsonObject),
            ResponseFormat::JsonSchema(schema) => Some(Self::JsonSchema {
                json_schema: JsonSchemaWire::from_domain(schema),
            }),
        }
    }
}

impl<'a> JsonSchemaWire<'a> {
    fn from_domain(schema: &'a StructuredSchema) -> Self {
        Self {
            name: schema.name(),
            description: schema.description(),
            schema: schema.schema().value(),
            strict: schema.strict(),
        }
    }
}
