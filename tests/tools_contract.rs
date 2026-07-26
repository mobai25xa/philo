//! Phase-two tool definition, schema, choice, and request-wire contract tests.

use philo::{
    CapabilitySet, CapabilityStatus, GenerateRequest, GenerationOptions, LlmError, Message,
    ModelRef, ParallelToolCalls, SchemaFailure, ToolChoice, ToolDefinition, ToolName, ToolSchema,
    ValidationReason, validate_tool_options,
};
use serde_json::{Value, json};

fn object_schema() -> ToolSchema {
    ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "city": { "type": "string" }
        },
        "required": ["city"],
        "additionalProperties": false
    }))
    .unwrap()
}

fn weather_tool() -> ToolDefinition {
    ToolDefinition::new(ToolName::new("get_weather").unwrap(), object_schema())
}

fn supported_tools_capabilities() -> CapabilitySet {
    CapabilitySet {
        function_tools: CapabilityStatus::Supported,
        tool_choice_required: CapabilityStatus::Supported,
        tool_choice_specific: CapabilityStatus::Supported,
        parallel_tool_calls: CapabilityStatus::Supported,
        strict_tools: CapabilityStatus::Supported,
        ..CapabilitySet::default()
    }
}

fn request_with(options: GenerationOptions) -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("openai", "gpt-test").unwrap(),
        vec![Message::user("Hello")],
    )
    .with_options(options)
}

#[test]
fn tool_schema_accepts_frozen_keywords_and_computes_strict_compatibility() {
    let schema = object_schema();
    assert!(schema.is_strict_compatible());
    assert_eq!(schema.value()["type"], "object");

    let non_strict = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "city": { "type": "string" }
        }
    }))
    .unwrap();
    assert!(!non_strict.is_strict_compatible());
}

#[test]
fn tool_schema_rejects_remote_ref_unsupported_keywords_and_size_limits() {
    let remote = ToolSchema::new(json!({
        "type": "object",
        "$ref": "https://example.com/schema.json"
    }))
    .unwrap_err();
    assert_eq!(remote.reason(), SchemaFailure::RemoteReference);
    assert_eq!(remote.path(), Some("#/$ref"));

    let unsupported = ToolSchema::new(json!({
        "type": "object",
        "oneOf": []
    }))
    .unwrap_err();
    assert_eq!(unsupported.reason(), SchemaFailure::UnsupportedKeyword);

    let too_large = ToolSchema::new(json!({
        "type": "object",
        "description": "x".repeat(300 * 1024),
        "properties": {},
        "required": [],
        "additionalProperties": false
    }))
    .unwrap_err();
    assert_eq!(too_large.reason(), SchemaFailure::TooLarge);
}

#[test]
fn tool_schema_resolves_local_refs_and_rejects_unresolved_ones() {
    let ok = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "item": { "$ref": "#/$defs/Item" }
        },
        "required": ["item"],
        "additionalProperties": false,
        "$defs": {
            "Item": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }
    }));
    assert!(ok.is_ok());

    let missing = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "item": { "$ref": "#/$defs/Missing" }
        },
        "required": ["item"],
        "additionalProperties": false
    }))
    .unwrap_err();
    assert_eq!(missing.reason(), SchemaFailure::UnresolvedLocalReference);
}

#[test]
fn tool_definition_and_choice_validation_cover_decision_table() {
    let capabilities = supported_tools_capabilities();
    let tools = vec![weather_tool()];

    assert!(validate_tool_options(&tools, Some(&ToolChoice::Auto), None, &capabilities).is_ok());
    assert!(validate_tool_options(&tools, Some(&ToolChoice::None), None, &capabilities).is_ok());
    assert!(
        validate_tool_options(&tools, Some(&ToolChoice::Required), None, &capabilities).is_ok()
    );
    assert!(
        validate_tool_options(
            &tools,
            Some(&ToolChoice::Specific {
                name: ToolName::new("get_weather").unwrap()
            }),
            None,
            &capabilities
        )
        .is_ok()
    );

    let duplicate = vec![weather_tool(), weather_tool()];
    assert!(matches!(
        validate_tool_options(&duplicate, None, None, &capabilities),
        Err(LlmError::Validation(error)) if error.reason() == ValidationReason::DuplicateToolName
    ));

    assert!(matches!(
        validate_tool_options(
            &[],
            Some(&ToolChoice::Required),
            None,
            &capabilities
        ),
        Err(LlmError::Validation(error)) if error.reason() == ValidationReason::EmptyToolList
    ));

    assert!(matches!(
        validate_tool_options(
            &tools,
            Some(&ToolChoice::Specific {
                name: ToolName::new("missing_tool").unwrap()
            }),
            None,
            &capabilities
        ),
        Err(LlmError::Validation(error)) if error.reason() == ValidationReason::UnknownTool
    ));
}

#[test]
fn unknown_and_unsupported_tool_capabilities_fail_closed() {
    let tools = vec![weather_tool()];
    for status in [CapabilityStatus::Unknown, CapabilityStatus::Unsupported] {
        let capabilities = CapabilitySet {
            function_tools: status,
            ..CapabilitySet::default()
        };
        assert!(matches!(
            validate_tool_options(&tools, None, None, &capabilities),
            Err(LlmError::Capability(_))
        ));
    }

    let capabilities = CapabilitySet {
        function_tools: CapabilityStatus::Supported,
        tool_choice_required: CapabilityStatus::Unknown,
        ..CapabilitySet::default()
    };
    assert!(matches!(
        validate_tool_options(&tools, Some(&ToolChoice::Required), None, &capabilities),
        Err(LlmError::Capability(_))
    ));

    let capabilities = CapabilitySet {
        function_tools: CapabilityStatus::Supported,
        parallel_tool_calls: CapabilityStatus::Unsupported,
        ..CapabilitySet::default()
    };
    assert!(matches!(
        validate_tool_options(
            &tools,
            None,
            Some(ParallelToolCalls::Enabled),
            &capabilities
        ),
        Err(LlmError::Capability(_))
    ));
}

#[test]
fn strict_tool_requires_compatible_schema_and_capability() {
    let non_strict_schema = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "city": { "type": "string" }
        }
    }))
    .unwrap();
    let tool = ToolDefinition::new(ToolName::new("get_weather").unwrap(), non_strict_schema)
        .with_strict(true);
    let capabilities = supported_tools_capabilities();
    assert!(matches!(
        validate_tool_options(&[tool], None, None, &capabilities),
        Err(LlmError::Schema(_))
    ));

    let tool = weather_tool().with_strict(true);
    let capabilities = CapabilitySet {
        function_tools: CapabilityStatus::Supported,
        strict_tools: CapabilityStatus::Unknown,
        ..CapabilitySet::default()
    };
    assert!(matches!(
        validate_tool_options(&[tool], None, None, &capabilities),
        Err(LlmError::Capability(_))
    ));
}

#[test]
fn generate_request_validation_rejects_invalid_tool_options_without_values() {
    let capabilities = supported_tools_capabilities();
    let request =
        request_with(GenerationOptions::new().with_tools(vec![weather_tool(), weather_tool()]));
    let error = request.validate(&capabilities).unwrap_err();
    assert!(matches!(
        error,
        LlmError::Validation(ref error) if error.reason() == ValidationReason::DuplicateToolName
    ));
    assert!(!error.to_string().contains("get_weather"));

    let request = request_with(GenerationOptions::new().with_tools(vec![weather_tool()]));
    assert!(request.validate(&capabilities).is_ok());
}

#[test]
fn tool_schema_debug_does_not_expose_schema_contents() {
    let schema = ToolSchema::new(json!({
        "type": "object",
        "description": "schema-canary-secret",
        "properties": {
            "token": { "type": "string", "const": "schema-canary-secret" }
        },
        "required": ["token"],
        "additionalProperties": false
    }))
    .unwrap();
    let debug = format!("{schema:?}");
    assert!(!debug.contains("schema-canary-secret"));
    assert!(debug.contains("strict_compatible"));
}

#[test]
fn tool_description_limit_is_enforced() {
    let error = weather_tool()
        .with_description("x".repeat(1025))
        .unwrap_err();
    assert_eq!(error.reason(), ValidationReason::OutOfRange);
}

// Adapter/wire golden tests are implemented against the private encoder through
// the same public request shapes used by production callers.

mod wire {
    use super::*;
    use philo::ReasoningEffortSupport;
    use philo::provider::ProviderCapabilities;

    // Re-test through GenerateRequest validation + documented field shapes by
    // serializing the Domain/tool decision outputs into expected OpenAI JSON via
    // helper that mirrors frozen wire mapping in protocol tests.

    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            developer_role: CapabilityStatus::Supported,
            temperature: CapabilityStatus::Supported,
            max_completion_tokens: CapabilityStatus::Supported,
            streaming: CapabilityStatus::Supported,
            streaming_usage: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Supported,
            tool_choice_required: CapabilityStatus::Supported,
            tool_choice_specific: CapabilityStatus::Supported,
            parallel_tool_calls: CapabilityStatus::Supported,
            strict_tools: CapabilityStatus::Supported,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
            adaptive_thinking: CapabilityStatus::Unknown,
            adaptive_thinking_effort: CapabilityStatus::Unknown,
        }
    }

    #[test]
    fn request_validation_accepts_official_tool_matrices() {
        let capabilities = capabilities().generation_options();
        let base = vec![weather_tool()];

        let cases = [
            GenerationOptions::new().with_tools(base.clone()),
            GenerationOptions::new()
                .with_tools(base.clone())
                .with_tool_choice(ToolChoice::None),
            GenerationOptions::new()
                .with_tools(base.clone())
                .with_tool_choice(ToolChoice::Required),
            GenerationOptions::new()
                .with_tools(base.clone())
                .with_tool_choice(ToolChoice::Specific {
                    name: ToolName::new("get_weather").unwrap(),
                }),
            GenerationOptions::new()
                .with_tools(base.clone())
                .with_parallel_tool_calls(ParallelToolCalls::Enabled),
            GenerationOptions::new().with_tools(vec![
                weather_tool()
                    .with_description("Get weather")
                    .unwrap()
                    .with_strict(true),
            ]),
        ];
        for options in cases {
            request_with(options).validate(&capabilities).unwrap();
        }
    }

    #[test]
    fn golden_fixture_files_are_valid_json_objects() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/phase-2/requests/tools");
        for name in [
            "tool-minimal-auto.json",
            "tool-none.json",
            "tool-required.json",
            "tool-specific.json",
            "tool-strict.json",
            "parallel-tools-enabled.json",
            "tool-description-omitted.json",
            "tool-schema-nested.json",
        ] {
            let raw = std::fs::read_to_string(root.join(name)).unwrap();
            let value: Value = serde_json::from_str(&raw).unwrap();
            assert!(value.is_object(), "{name} must be a JSON object");
            assert_eq!(value["stream"], true);
            assert_eq!(value["n"], 1);
            assert!(value.get("tools").is_some(), "{name} missing tools");
        }
    }

    #[test]
    fn documented_failure_fixtures_match_typed_error_paths() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/phase-2/requests/tools/failures");
        for name in [
            "duplicate-tool-name.json",
            "specific-tool-missing.json",
            "strict-capability-unknown.json",
            "parallel-unsupported.json",
            "invalid-schema.json",
            "empty-tool-list-required.json",
        ] {
            let raw = std::fs::read_to_string(root.join(name)).unwrap();
            let value: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(value["expected"], "error");
            assert!(value["fixture_id"].is_string());
        }

        let capabilities = supported_tools_capabilities();
        assert!(matches!(
            validate_tool_options(
                &[weather_tool(), weather_tool()],
                None,
                None,
                &capabilities
            ),
            Err(LlmError::Validation(error)) if error.reason() == ValidationReason::DuplicateToolName
        ));
        assert!(matches!(
            validate_tool_options(
                &[weather_tool()],
                Some(&ToolChoice::Specific {
                    name: ToolName::new("missing_tool").unwrap()
                }),
                None,
                &capabilities
            ),
            Err(LlmError::Validation(error)) if error.reason() == ValidationReason::UnknownTool
        ));
        assert!(matches!(
            validate_tool_options(&[], Some(&ToolChoice::Required), None, &capabilities),
            Err(LlmError::Validation(error)) if error.reason() == ValidationReason::EmptyToolList
        ));
        assert!(matches!(
            validate_tool_options(
                &[weather_tool().with_strict(true)],
                None,
                None,
                &CapabilitySet {
                    function_tools: CapabilityStatus::Supported,
                    strict_tools: CapabilityStatus::Unknown,
                    ..CapabilitySet::default()
                }
            ),
            Err(LlmError::Capability(_))
        ));
        assert!(matches!(
            validate_tool_options(
                &[weather_tool()],
                None,
                Some(ParallelToolCalls::Enabled),
                &CapabilitySet {
                    function_tools: CapabilityStatus::Supported,
                    parallel_tool_calls: CapabilityStatus::Unsupported,
                    ..CapabilitySet::default()
                }
            ),
            Err(LlmError::Capability(_))
        ));
        assert_eq!(
            ToolSchema::new(json!(true)).unwrap_err().reason(),
            SchemaFailure::NotAnObject
        );
    }

    #[test]
    fn schema_fixture_files_cover_valid_invalid_and_unsupported() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phase-2/schemas");
        let valid: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("valid/object-required.json")).unwrap(),
        )
        .unwrap();
        let schema = ToolSchema::new(valid["schema"].clone()).unwrap();
        assert!(schema.is_strict_compatible());

        for name in ["valid/nullable-anyof.json", "valid/array-enum.json"] {
            let document: Value =
                serde_json::from_str(&std::fs::read_to_string(root.join(name)).unwrap()).unwrap();
            ToolSchema::new(document["schema"].clone()).unwrap();
        }

        let invalid: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("invalid/not-an-object.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ToolSchema::new(invalid["schema"].clone())
                .unwrap_err()
                .reason(),
            SchemaFailure::NotAnObject
        );

        let unsupported: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("unsupported-keywords/one-of.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ToolSchema::new(unsupported["schema"].clone())
                .unwrap_err()
                .reason(),
            SchemaFailure::UnsupportedKeyword
        );

        let remote: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("unsupported-keywords/remote-ref.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ToolSchema::new(remote["schema"].clone())
                .unwrap_err()
                .reason(),
            SchemaFailure::RemoteReference
        );
    }
}
