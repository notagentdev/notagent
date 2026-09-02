//! Verifies the embedded model-catalog snapshot against its manifest.
//! manifest validation (SHA-256 per file, structureHash, required fields per model) to
//! be reproduced as a test.

use std::collections::{BTreeMap, BTreeSet};

use notagent_ai::model_catalog::*;
use notagent_ai::types::Model;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn manifest() -> Value {
    serde_json::from_str(MODEL_DATA_MANIFEST).expect("manifest is valid JSON")
}

#[test]
fn manifest_has_schema_version_three_and_a_structure_hash() {
    let manifest = manifest();
    assert_eq!(manifest["schemaVersion"], 3);
    assert_eq!(
        manifest["structureHash"].as_str().map(str::len),
        Some(64),
        "structureHash is a SHA-256 hex digest"
    );
    assert!(manifest["generatedAt"].as_str().is_some());
    assert!(get_builtin_model_data_generated_at().is_some());
}

#[test]
fn every_embedded_file_matches_its_manifest_digest() {
    let manifest = manifest();
    let files = manifest["files"].as_object().expect("files map");
    assert_eq!(
        files.len(),
        MODEL_DATA.len(),
        "every embedded provider has one manifest entry"
    );

    for (provider, raw) in MODEL_DATA {
        let file_name = format!("{provider}.json");
        let expected = files
            .get(&file_name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{file_name} is missing from the manifest"));
        let digest = Sha256::digest(raw.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            digest, expected,
            "{file_name} does not match its manifest digest"
        );
    }

    let embedded: BTreeSet<String> = MODEL_DATA
        .iter()
        .map(|(provider, _)| format!("{provider}.json"))
        .collect();
    let listed: BTreeSet<String> = files.keys().cloned().collect();
    assert_eq!(
        embedded, listed,
        "the embedded files and the manifest must agree"
    );
}

#[test]
fn the_manifest_structure_hash_matches_the_embedded_catalog() {
    let mut structure = BTreeMap::<&str, BTreeMap<String, String>>::new();
    let mut model_count = 0;
    for (provider, raw) in MODEL_DATA {
        let groups: Value = serde_json::from_str(raw).expect("valid JSON");
        let mut models = BTreeMap::new();
        for (api, entries) in groups.as_object().expect("api groups") {
            for model_id in entries.as_object().expect("model entries").keys() {
                assert!(
                    models.insert(model_id.clone(), api.clone()).is_none(),
                    "{provider}/{model_id} appears in more than one API group"
                );
                model_count += 1;
            }
        }
        structure.insert(provider, models);
    }

    let digest = Sha256::digest(
        serde_json::to_vec(&structure).expect("the catalog structure serializes deterministically"),
    )
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect::<String>();
    assert_eq!(
        manifest()["structureHash"].as_str(),
        Some(digest.as_str()),
        "the generation stamp must describe the embedded catalog"
    );
    assert_eq!(get_builtin_providers().len(), MODEL_DATA.len());
    assert_eq!(all_builtin_models().len(), model_count);
}

#[test]
fn every_model_carries_the_required_fields() {
    for model in all_builtin_models() {
        assert!(!model.id.is_empty(), "model without id");
        assert!(!model.name.is_empty(), "{} has no name", model.id);
        assert!(!model.api.is_empty(), "{} has no api", model.id);
        assert!(!model.provider.is_empty(), "{} has no provider", model.id);
        // Azure models carry an empty baseUrl on purpose: the endpoint is configured
        // per deployment (38 models in the snapshot).
        assert!(
            !model.base_url.is_empty() || model.provider == "azure-openai-responses",
            "{} has no baseUrl",
            model.id
        );
        assert!(
            !model.input.is_empty(),
            "{} has no input modalities",
            model.id
        );
        assert!(
            model.context_window > 0,
            "{} has no context window",
            model.id
        );
        assert!(model.max_tokens > 0, "{} has no max tokens", model.id);
    }
}

#[test]
fn models_carry_the_provider_of_their_file() {
    for (provider, _) in MODEL_DATA {
        for model in get_builtin_models(provider) {
            assert_eq!(
                model.provider, provider,
                "{} is filed under {provider}",
                model.id
            );
        }
    }
}

#[test]
fn parsed_models_round_trip_back_to_the_snapshot_json() {
    for (provider, raw) in MODEL_DATA {
        let groups: Value = serde_json::from_str(raw).expect("valid JSON");
        for (_api, entries) in groups.as_object().expect("api groups") {
            for (id, original) in entries.as_object().expect("model entries") {
                let model: Model = serde_json::from_value(original.clone())
                    .unwrap_or_else(|error| panic!("{provider}/{id}: {error}"));
                let reserialized = serde_json::to_value(&model).expect("serialize");
                assert_eq!(
                    &reserialized, original,
                    "{provider}/{id} does not round-trip"
                );
            }
        }
    }
}

#[test]
fn known_models_are_reachable_by_id() {
    let model = get_builtin_model("anthropic", "claude-fable-5").expect("claude-fable-5");
    assert_eq!(model.api, "anthropic-messages");
    assert!(model.reasoning);
    assert!(get_builtin_model("anthropic", "does-not-exist").is_none());
}
