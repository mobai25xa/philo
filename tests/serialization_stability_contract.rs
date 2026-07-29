//! Complete inventory for every production serde type.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct Inventory {
    schema_version: u32,
    reviewed_at: String,
    group: Vec<SerializationGroup>,
}

#[derive(Deserialize)]
struct SerializationGroup {
    source: String,
    classification: String,
    stability: String,
    format_id: Option<String>,
    format_version: Option<String>,
    migration: String,
    types: Vec<String>,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

fn serde_types() -> BTreeSet<(String, String)> {
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);
    rust_files(&root().join("crates"), &mut files);
    let mut found = BTreeSet::new();

    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            if !line.trim_start().starts_with("#[derive(")
                || (!line.contains("Serialize") && !line.contains("Deserialize"))
            {
                continue;
            }
            let declaration = lines
                .iter()
                .skip(index + 1)
                .take(6)
                .map(|line| line.trim())
                .find(|line| line.contains("struct ") || line.contains("enum "))
                .unwrap_or_else(|| panic!("serde derive has no nearby type in {}", file.display()));
            let marker = if declaration.contains("struct ") {
                "struct "
            } else {
                "enum "
            };
            let name = declaration
                .split(marker)
                .nth(1)
                .unwrap()
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .next()
                .unwrap()
                .to_owned();
            let source = file
                .strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            assert!(
                found.insert((source, name)),
                "duplicate serde type discovery"
            );
        }
    }
    found
}

#[test]
fn every_serde_type_has_exactly_one_stability_classification() {
    let text =
        fs::read_to_string(root().join("docs/maintenance/serialization-inventory.toml")).unwrap();
    let inventory: Inventory = toml::from_str(&text).unwrap();
    assert_eq!(inventory.schema_version, 1);
    assert_eq!(inventory.reviewed_at.len(), 10);

    let allowed_classes = BTreeSet::from([
        "Public Stable Persistence",
        "Public Diagnostic",
        "Wire Protocol",
        "Internal Cache/State",
        "Test Fixture",
    ]);
    let allowed_stability = BTreeSet::from(["Stable", "Experimental", "Internal"]);
    let mut classified = BTreeMap::new();

    for group in inventory.group {
        assert!(
            root().join(&group.source).is_file(),
            "missing source {}",
            group.source
        );
        assert!(allowed_classes.contains(group.classification.as_str()));
        assert!(allowed_stability.contains(group.stability.as_str()));
        assert!(!group.migration.trim().is_empty());
        if group.classification == "Public Stable Persistence" {
            assert!(
                group
                    .format_id
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
            );
            assert!(
                group
                    .format_version
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
            );
        }
        for type_name in group.types {
            let key = (group.source.clone(), type_name);
            assert!(
                classified
                    .insert(key.clone(), group.classification.clone())
                    .is_none(),
                "duplicate inventory entry {key:?}"
            );
        }
    }

    let discovered = serde_types();
    assert_eq!(
        classified.keys().cloned().collect::<BTreeSet<_>>(),
        discovered
    );
}
