//! A consumer-side encoder: no AWR hash or canonicalization helpers.
use serde_json::Value;
use sha2::{Digest, Sha256};

fn sorted_json(value: &Value) -> String {
    match value {
        Value::Object(fields) => {
            let mut names: Vec<_> = fields.keys().collect();
            names.sort();
            let entries = names
                .into_iter()
                .map(|name| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(name).unwrap(),
                        sorted_json(&fields[name])
                    )
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", entries.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(sorted_json).collect::<Vec<_>>().join(",")
        ),
        _ => serde_json::to_string(value).unwrap(),
    }
}

pub fn digest_json(value: &Value) -> String {
    format!("{:x}", Sha256::digest(sorted_json(value).as_bytes()))
}

pub fn manifest_digest(description: &Value, manifest: &Value) -> String {
    let codec = &description["manifest_hash_codec"];
    assert_eq!(codec["algorithm"], "sha256");
    assert_eq!(codec["digest_encoding"], "lowercase hexadecimal");
    let mut preimage = codec["envelope"].clone();
    *preimage
        .pointer_mut(codec["manifest_pointer"].as_str().unwrap())
        .unwrap() = manifest.clone();
    digest_json(&preimage)
}
