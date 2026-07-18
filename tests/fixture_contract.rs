//! Machine-readable fixture metadata, provenance, and safety contracts.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use philo::{
    BodySummary, ByteStream, PHASE_ONE_CONTRACT_ID, PHASE_ONE_CONTRACT_VERSION, SseDecoder,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureManifest {
    schema_version: u32,
    contract_id: String,
    contract_version: String,
    fixture: Vec<FixtureEntry>,
}

#[derive(Deserialize)]
struct FixtureEntry {
    id: String,
    path: String,
    purpose: String,
    source: String,
    source_url: Option<String>,
    captured_at: Option<String>,
    sanitized_at: Option<String>,
    expected: String,
    expected_error: Option<String>,
    contract_version: String,
    notes: String,
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_manifest() -> FixtureManifest {
    let text = fs::read_to_string(fixture_root().join("manifest.toml")).unwrap();
    toml::from_str(&text).unwrap()
}

fn collect_files(root: &Path, directory: &Path, output: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(root, &path, output);
        } else if path
            .file_name()
            .is_some_and(|name| name != "README.md" && name != "manifest.toml")
        {
            output.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

#[test]
fn every_fixture_is_uniquely_described_and_present() {
    let root = fixture_root();
    let manifest = read_manifest();
    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.contract_id, PHASE_ONE_CONTRACT_ID);
    assert_eq!(manifest.contract_version, PHASE_ONE_CONTRACT_VERSION);

    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::<String>::new();
    for fixture in &manifest.fixture {
        assert!(
            ids.insert(&fixture.id),
            "duplicate fixture id: {}",
            fixture.id
        );
        assert!(
            paths.insert(fixture.path.clone()),
            "duplicate fixture path: {}",
            fixture.path
        );
        assert!(!fixture.purpose.trim().is_empty());
        assert!(!fixture.notes.trim().is_empty());
        assert_eq!(fixture.contract_version, PHASE_ONE_CONTRACT_VERSION);
        assert!(matches!(
            fixture.source.as_str(),
            "synthetic" | "official-doc-example" | "sanitized-observation"
        ));
        assert!(matches!(fixture.expected.as_str(), "success" | "error"));
        assert_eq!(
            fixture.expected == "error",
            fixture.expected_error.is_some()
        );
        if fixture.source == "sanitized-observation" {
            assert!(fixture.source_url.is_some());
            assert!(fixture.captured_at.is_some());
            assert!(fixture.sanitized_at.is_some());
        }

        let relative = Path::new(&fixture.path);
        assert!(!relative.is_absolute());
        assert!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        );
        assert!(
            root.join(relative).is_file(),
            "missing fixture: {}",
            fixture.path
        );
    }

    let mut actual = BTreeSet::new();
    collect_files(&root, &root, &mut actual);
    assert_eq!(paths, actual, "manifest and fixture tree differ");
}

#[test]
fn fixture_tree_contains_no_credential_canaries() {
    let root = fixture_root();
    let manifest = read_manifest();
    let forbidden = ["sk-", "bearer philo-", "api_key=", "access_token="];
    for fixture in manifest.fixture {
        let bytes = fs::read(root.join(&fixture.path)).unwrap();
        let searchable = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
        for marker in forbidden {
            assert!(
                !searchable.contains(marker),
                "credential marker in fixture {}",
                fixture.id
            );
        }
    }
}

#[tokio::test]
async fn encoded_binary_and_crlf_fixtures_replay_deterministically() {
    let root = fixture_root();
    let hex = fs::read_to_string(root.join("errors/non-utf8.hex")).unwrap();
    let binary: Vec<u8> = hex
        .trim()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    assert!(std::str::from_utf8(&binary).is_err());
    assert!(
        BodySummary::from_bytes(&binary, binary.len())
            .as_str()
            .contains('\u{fffd}')
    );

    let escaped =
        fs::read_to_string(root.join("responses/openai_chat/crlf-heartbeat.escaped-sse")).unwrap();
    let replay = escaped.replace("\\r", "\r").replace("\\n", "\n");
    assert!(replay.contains("\r\n"));
    let chunks: Vec<_> = replay
        .as_bytes()
        .chunks(7)
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
        .collect();
    let body: ByteStream = Box::pin(stream::iter(chunks));
    let events = SseDecoder::new(body)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[0].data().contains('\n'));
    assert_eq!(events[2].data(), "[DONE]");
}

#[test]
fn profile_fixtures_freeze_official_and_test_only_boundaries() {
    let root = fixture_root();
    let official: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("profiles/official-openai.toml")).unwrap())
            .unwrap();
    assert_eq!(
        official["base_url"].as_str(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(official["path"].as_str(), Some("/chat/completions"));
    assert_eq!(
        official["api_key_source"].as_str(),
        Some("environment:OPENAI_API_KEY")
    );
    assert_eq!(official["test_only"].as_bool(), Some(false));

    let local: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("profiles/local-test-only.toml")).unwrap())
            .unwrap();
    assert_eq!(local["kind"].as_str(), Some("local-test-only"));
    assert!(
        local["endpoint"]
            .as_str()
            .is_some_and(|endpoint| endpoint.starts_with("http://127.0.0.1:"))
    );
    assert_eq!(local["test_only"].as_bool(), Some(true));
}
