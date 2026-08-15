use std::{collections::HashSet, fs, path::Path};

use serde_json::Value;

const MINIMUM_BOUNDARIES: usize = 8;
const MINIMUM_IDENTITIES: usize = 7;
const MINIMUM_DATA_CLASSES: usize = 8;
const MINIMUM_ABUSE_CASES: usize = 16;

#[test]
fn security_threat_model_is_complete_and_reviewable() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let registry_path = workspace.join("security/threat-model.json");
    let document_path = workspace.join("docs/threat-model.md");
    let registry_text = fs::read_to_string(registry_path).expect("read threat-model registry");
    let document = fs::read_to_string(document_path).expect("read threat-model document");
    let registry: Value =
        serde_json::from_str(&registry_text).expect("parse threat-model registry");

    assert_eq!(registry["version"].as_u64(), Some(1));
    assert!(document.contains("Version 1, reviewed"));
    assert!(document.contains("## Security invariants"));
    assert!(document.contains("## Detection and response"));
    assert!(document.contains("## Residual risk and review triggers"));

    let boundaries = required_array(&registry, "trust_boundaries", MINIMUM_BOUNDARIES);
    let identities = required_array(&registry, "privileged_identities", MINIMUM_IDENTITIES);
    let data_classes = required_array(&registry, "sensitive_data_classes", MINIMUM_DATA_CLASSES);
    let abuse_cases = required_array(&registry, "abuse_cases", MINIMUM_ABUSE_CASES);

    let boundary_ids = validate_catalog(boundaries, "TB-", &["name", "entry_controls"], &document);
    let identity_ids = validate_catalog(
        identities,
        "PI-",
        &["name", "least_privilege", "credential"],
        &document,
    );
    let data_class_ids = validate_catalog(
        data_classes,
        "SD-",
        &["name", "handling", "retention"],
        &document,
    );
    let abuse_case_ids = validate_catalog(
        abuse_cases,
        "AC-",
        &[
            "title",
            "owner",
            "assets",
            "boundaries",
            "privileged_identities",
            "prevent",
            "detect",
            "respond",
            "verify",
        ],
        &document,
    );

    assert_eq!(abuse_case_ids.len(), abuse_cases.len());
    for abuse_case in abuse_cases {
        let id = required_string(abuse_case, "id");
        validate_references(abuse_case, id, "assets", &data_class_ids);
        validate_references(abuse_case, id, "boundaries", &boundary_ids);
        validate_references(abuse_case, id, "privileged_identities", &identity_ids);
        for verification in required_string_array(abuse_case, "verify") {
            assert!(
                verification.starts_with("SEC-"),
                "{id} verification ID must start with SEC-: {verification}"
            );
        }
    }
}

fn required_array<'a>(root: &'a Value, field: &str, minimum: usize) -> &'a [Value] {
    let values = root[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} must be an array"));
    assert!(
        values.len() >= minimum,
        "{field} must contain at least {minimum} entries"
    );
    values
}

fn validate_catalog(
    values: &[Value],
    prefix: &str,
    required_fields: &[&str],
    document: &str,
) -> HashSet<String> {
    let mut ids = HashSet::new();
    for value in values {
        let id = required_string(value, "id");
        assert!(id.starts_with(prefix), "{id} must start with {prefix}");
        assert!(ids.insert(id.to_owned()), "duplicate threat-model ID {id}");
        assert!(
            document.contains(&format!("`{id}`")),
            "{id} is missing from docs/threat-model.md"
        );
        for field in required_fields {
            assert_present(value, id, field);
        }
    }
    ids
}

fn assert_present(value: &Value, id: &str, field: &str) {
    let present = match &value[field] {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(values) => {
            !values.is_empty()
                && values
                    .iter()
                    .all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty()))
        }
        _ => false,
    };
    assert!(present, "{id}.{field} must be non-empty text or text array");
}

fn required_string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| panic!("{field} must be non-empty text"))
}

fn required_string_array<'a>(value: &'a Value, field: &str) -> impl Iterator<Item = &'a str> {
    value[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} must be an array"))
        .iter()
        .map(move |item| {
            item.as_str()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| panic!("{field} must contain non-empty text"))
        })
}

fn validate_references(value: &Value, id: &str, field: &str, known: &HashSet<String>) {
    for reference in required_string_array(value, field) {
        assert!(
            known.contains(reference),
            "{id}.{field} references unknown ID {reference}"
        );
    }
}
