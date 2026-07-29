//! Versioned provider configuration, deterministic merge, and secret-boundary contracts.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use http::{HeaderMap, header};
use philo::error::{ProviderConfigError, ProviderConfigFailure};
use philo::provider::auth::ApiKey;
use philo::provider::profiles::OfficialOpenAiProfile;
use philo::provider::secret::{SecretReference, SecretResolver};
use philo_config::{
    ClientIdentityConfig, ConfigSchemaVersion, ConfigSource, ConfigSourceKind, ConfigValue,
    EndpointSpec, FieldState, ProviderConfigDocument, ProviderConfigField, ProviderConfigLayer,
    ProviderConfigSnapshot,
};

const KEY_CANARY: &str = "philo-config-secret-canary-1734";
const SECRET_NAME: &str = "PHILO_OPENAI_API_KEY";

fn fixture(path: &str) -> String {
    fs::read_to_string(fixture_root().join(path)).unwrap()
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-config")
}

fn environment_layer() -> ProviderConfigLayer {
    ProviderConfigLayer::new(ConfigSource::environment_secret("env/openai", SECRET_NAME).unwrap())
        .with_credential(ConfigValue::set(
            SecretReference::environment_variable(SECRET_NAME).unwrap(),
        ))
}

#[derive(Default)]
struct CountingResolver {
    calls: Cell<usize>,
}

impl SecretResolver for CountingResolver {
    fn resolve(&self, reference: &SecretReference) -> Result<ApiKey, ProviderConfigError> {
        self.calls.set(self.calls.get() + 1);
        if reference.name() != SECRET_NAME {
            return Err(ProviderConfigError::new(
                "credential",
                ProviderConfigFailure::SecretUnavailable,
                "test resolver has no matching secret",
            ));
        }
        ApiKey::new(KEY_CANARY).map_err(|_| {
            ProviderConfigError::new(
                "credential",
                ProviderConfigFailure::InvalidValue,
                "test secret is invalid",
            )
        })
    }
}

#[test]
fn same_sources_always_produce_same_snapshot() {
    let user =
        ProviderConfigLayer::new(ConfigSource::user_config("file/app", "provider.json").unwrap())
            .with_client_identity(ConfigValue::set(ClientIdentityConfig::new(
                "my-app", "2.0.0",
            )));
    let programmatic =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/bootstrap").unwrap())
            .with_max_http_error_body_bytes(ConfigValue::set(8192));

    let first = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([user.clone(), environment_layer(), programmatic.clone()])
        .unwrap();
    let second = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([programmatic, user, environment_layer()])
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.version(), ConfigSchemaVersion::CURRENT);
}

#[test]
fn source_precedence_is_fieldwise_and_traceable() {
    let user = ProviderConfigLayer::new(
        ConfigSource::user_config("file/limits", "provider.json").unwrap(),
    )
    .with_max_http_error_body_bytes(ConfigValue::set(4096));
    let programmatic =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/override").unwrap())
            .with_max_http_error_body_bytes(ConfigValue::set(8192));

    let snapshot = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([programmatic, user, environment_layer()])
        .unwrap();
    assert_eq!(snapshot.max_http_error_body_bytes(), Some(8192));
    let limit_source = snapshot
        .provenance(ProviderConfigField::MaxHttpErrorBodyBytes)
        .unwrap();
    assert_eq!(
        limit_source.source().kind(),
        ConfigSourceKind::ProgrammaticOverride
    );
    assert_eq!(limit_source.source().id().as_str(), "application/override");
    assert_eq!(limit_source.state(), FieldState::Set);

    let credential_source = snapshot
        .provenance(ProviderConfigField::Credential)
        .unwrap();
    assert_eq!(
        credential_source.source().kind(),
        ConfigSourceKind::EnvironmentSecretReference
    );
    assert_eq!(snapshot.credential_reference().unwrap().name(), SECRET_NAME);
}

#[test]
fn unset_remove_and_empty_are_not_conflated() {
    let unset =
        ProviderConfigLayer::new(ConfigSource::user_config("file/unset", "provider.json").unwrap());
    let unchanged = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([unset])
        .unwrap();
    assert_eq!(unchanged.provider_id(), Some("official-openai"));

    let removed =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/remove").unwrap())
            .with_provider_id(ConfigValue::remove());
    let error = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([removed])
        .unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::MissingRequiredField);
    assert_eq!(error.source(), Some("application/remove"));

    let empty = ProviderConfigLayer::new(ConfigSource::programmatic("application/empty").unwrap())
        .with_provider_id(ConfigValue::set(String::new()));
    let error = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([empty])
        .unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::InvalidValue);
    assert_eq!(error.source(), Some("application/empty"));
}

#[test]
fn unknown_major_unknown_field_and_invalid_cross_field_fail_before_secret_resolution() {
    let error = ProviderConfigDocument::from_json(&fixture("unknown-major.json")).unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::InvalidVersion);

    let error = ProviderConfigLayer::from_json(
        &fixture("unknown-field.json"),
        ConfigSource::user_config("file/unknown", "unknown-field.json").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::InvalidDocument);
    assert_eq!(error.source(), Some("file/unknown"));
    assert!(!error.to_string().contains("extra_body"));

    let newer_minor = ProviderConfigLayer::from_json(
        r#"{"schema_version":{"major":1,"minor":7}}"#,
        ConfigSource::user_config("file/minor", "minor.json").unwrap(),
    )
    .unwrap();
    let snapshot = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([newer_minor])
        .unwrap();
    assert_eq!(snapshot.version().minor, 7);

    let unsafe_endpoint =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/endpoint").unwrap())
            .with_endpoint(ConfigValue::set(EndpointSpec::base_and_path(
                "https://example.com/v1",
                "/chat/completions",
            )));
    let snapshot = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([environment_layer(), unsafe_endpoint])
        .unwrap();
    let resolver = CountingResolver::default();
    let error = snapshot
        .build_official_openai_runtime(&resolver)
        .unwrap_err();
    assert!(matches!(error, philo::LlmError::ProviderConfig(_)));
    assert_eq!(resolver.calls.get(), 0);
}

#[test]
fn previous_minor_is_migrated_and_writer_emits_only_current_schema() {
    let previous = ProviderConfigDocument::from_json(&fixture("official-user.json")).unwrap();
    assert_eq!(previous.schema_version, ConfigSchemaVersion::CURRENT);

    let json = previous.to_current_json().unwrap();
    assert!(json.contains("\"minor\": 1"));
    let roundtrip = ProviderConfigDocument::from_json(&json).unwrap();
    assert_eq!(roundtrip, previous);
    assert_eq!(
        ConfigSchemaVersion::PREVIOUS.minor + 1,
        ConfigSchemaVersion::CURRENT.minor
    );
}

#[test]
fn secret_reference_never_formats_resolved_value() {
    let document = ProviderConfigDocument::from_json(&fixture("official-user.json")).unwrap();
    let layer = ProviderConfigLayer::from_document(
        document,
        ConfigSource::user_config("file/official", "official-user.json").unwrap(),
    )
    .unwrap();
    let snapshot = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([layer])
        .unwrap();
    let debug = format!("{snapshot:?}");
    assert!(!debug.contains(KEY_CANARY));

    let resolver = CountingResolver::default();
    let runtime = snapshot.build_official_openai_runtime(&resolver).unwrap();
    assert_eq!(resolver.calls.get(), 1);
    assert!(!format!("{runtime:?}").contains(KEY_CANARY));
}

#[test]
fn environment_and_per_request_sources_cannot_modify_provider_security_fields() {
    let environment =
        ProviderConfigLayer::new(ConfigSource::environment_secret("env/bad", SECRET_NAME).unwrap())
            .with_endpoint(ConfigValue::set(EndpointSpec::absolute(
                "https://example.com/v1/chat/completions",
            )));
    let error = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([environment])
        .unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::ForbiddenOverride);

    let per_request =
        ProviderConfigLayer::new(ConfigSource::per_request("request/override").unwrap())
            .with_provider_id(ConfigValue::set("attacker".to_owned()));
    let error = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([per_request])
        .unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::ForbiddenOverride);

    let mismatched = ProviderConfigLayer::new(
        ConfigSource::environment_secret("env/mismatch", "OTHER_KEY").unwrap(),
    )
    .with_credential(ConfigValue::set(
        SecretReference::environment_variable(SECRET_NAME).unwrap(),
    ));
    let error = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([mismatched])
        .unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::MergeConflict);
}

#[test]
fn official_openai_legacy_constructor_matches_compiled_preset() {
    let legacy = OfficialOpenAiProfile::from_api_key(KEY_CANARY)
        .unwrap()
        .build()
        .unwrap();
    let resolver = CountingResolver::default();
    let compiled = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([environment_layer()])
        .unwrap()
        .build_official_openai_runtime(&resolver)
        .unwrap();

    assert_eq!(compiled.provider_id(), legacy.provider_id());
    assert_eq!(compiled.protocol_id(), legacy.protocol_id());
    assert_eq!(compiled.endpoint(), legacy.endpoint());
    assert_eq!(compiled.capabilities(), legacy.capabilities());
    assert_eq!(compiled.dialect(), legacy.dialect());
    assert_eq!(compiled.transport_options(), legacy.transport_options());

    let compiled_headers = compiled
        .resolve_headers(Vec::new(), &HeaderMap::new())
        .unwrap();
    let legacy_headers = legacy
        .resolve_headers(Vec::new(), &HeaderMap::new())
        .unwrap();
    for name in [
        header::CONTENT_TYPE,
        header::ACCEPT,
        header::USER_AGENT,
        header::AUTHORIZATION,
    ] {
        assert_eq!(
            compiled_headers.headers().get(&name),
            legacy_headers.headers().get(&name)
        );
    }
}

#[test]
fn duplicate_source_identity_is_rejected_instead_of_using_input_order() {
    let first =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/duplicate").unwrap())
            .with_max_http_error_body_bytes(ConfigValue::set(4096));
    let second =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/duplicate").unwrap())
            .with_max_http_error_body_bytes(ConfigValue::set(8192));
    let error = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([first, second])
        .unwrap_err();
    assert_eq!(error.reason(), ProviderConfigFailure::MergeConflict);
}

/// Moved from the core's security-hardening suite with FR-005: bounding a
/// configuration document is the configuration layer's job now, but the bound
/// itself must not weaken.
#[test]
fn oversized_and_unknown_configuration_fail_before_resolution() {
    let oversized = format!(
        "{{\"schema_version\":{{\"major\":1,\"minor\":0}},\"padding\":\"{}\"}}",
        "x".repeat(64 * 1024)
    );
    assert!(ProviderConfigDocument::from_json(&oversized).is_err());
    assert!(
        ProviderConfigDocument::from_json(
            r#"{"schema_version":{"major":1,"minor":0},"unexpected":true}"#,
        )
        .is_err()
    );
}

/// The fixture manifest moved out of the core with the module that reads it
/// (FR-005). Same guarantee, new owner: every declared fixture exists, the tree
/// declares nothing extra, and no fixture carries credential material.
#[test]
fn the_moved_fixture_manifest_stays_complete_and_credential_free() {
    let root = fixture_root().parent().unwrap().to_path_buf();
    let manifest = fs::read_to_string(root.join("manifest.toml")).unwrap();
    let parsed: toml::Value = toml::from_str(&manifest).unwrap();
    assert_eq!(parsed["schema_version"].as_integer(), Some(3));
    let entries = parsed["fixture"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    for entry in entries {
        assert_eq!(entry["contract_id"].as_str(), Some("philo/provider-config"));
        assert_eq!(entry["contract_version"].as_str(), Some("1.1"));
        assert_eq!(entry["source"].as_str(), Some("synthetic"));
        assert_eq!(entry["public_allowed"].as_bool(), Some(true));
        for field in [
            "category",
            "protocol",
            "captured_at",
            "reviewed_at",
            "redaction_status",
            "sanitized_at",
            "license_or_permission",
            "expected_summary",
        ] {
            assert!(
                entry[field].as_str().is_some(),
                "missing fixture field {field}"
            );
        }
    }

    let mut declared = manifest
        .lines()
        .filter_map(|line| line.trim().strip_prefix("path = \""))
        .filter_map(|line| line.strip_suffix('"'))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    declared.sort();
    assert_eq!(
        declared,
        vec![
            "provider-config/official-user.json",
            "provider-config/unknown-field.json",
            "provider-config/unknown-major.json",
        ]
    );

    let mut present = Vec::new();
    for entry in fs::read_dir(root.join("provider-config")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let body = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        for marker in ["sk-", "bearer philo-", "api_key=", "access_token="] {
            assert!(!body.contains(marker), "credential marker in {name}");
        }
        present.push(format!("provider-config/{name}"));
    }
    present.sort();
    assert_eq!(present, declared, "manifest and fixture tree differ");
}
