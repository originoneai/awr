//! Evidence encodings converge on the same exact bytes before hashing/storage.
use super::*;

const MAX_BYTES: usize = crate::WorkstreamQuery::MAX_ARTIFACT_BYTES;

/// The typed delivery evidence payload is a real JSON object whose declared
/// fields match the published schema. Unknown keys are preserved untouched and
/// never walked; the object itself is stored and hashed exactly as submitted.
pub(super) fn validate_delivery_payload(payload: &Value) -> PgResult<()> {
    let Some(object) = payload.as_object() else {
        return Err(PgError::delivery_payload_invalid(
            "delivery_payload",
            "type",
        ));
    };
    match object.get("delivery_candidate_digest") {
        None => {
            return Err(PgError::delivery_payload_invalid(
                "delivery_candidate_digest",
                "required",
            ));
        }
        Some(value) if !value.as_str().is_some_and(digest) => {
            return Err(PgError::delivery_payload_invalid(
                "delivery_candidate_digest",
                "pattern",
            ));
        }
        _ => {}
    }
    if let Some(passed) = object.get("passed") {
        if !passed.is_boolean() {
            return Err(PgError::delivery_payload_invalid("passed", "type"));
        }
    }
    if let Some(output) = object.get("output_digest") {
        match output.as_str() {
            None => return Err(PgError::delivery_payload_invalid("output_digest", "type")),
            Some(text) if !digest(text) => {
                return Err(PgError::delivery_payload_invalid(
                    "output_digest",
                    "pattern",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn decode(hex: Option<&str>, text: Option<&str>) -> PgResult<Option<Vec<u8>>> {
    match (hex, text) {
        (Some(_), Some(_)) => Err(invalid()),
        (None, None) => Ok(None),
        (None, Some(text)) => {
            if text.len() > MAX_BYTES {
                return Err(invalid());
            }
            // Preserve whitespace, line endings and Unicode normalization.
            Ok(Some(text.as_bytes().to_vec()))
        }
        (Some(hex), None) => {
            if hex.len() > MAX_BYTES * 2
                || hex.len() % 2 != 0
                || !hex.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid());
            }
            let bytes = hex
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let hi = (pair[0] as char).to_digit(16).ok_or_else(invalid)? as u8;
                    let lo = (pair[1] as char).to_digit(16).ok_or_else(invalid)? as u8;
                    Ok((hi << 4) | lo)
                })
                .collect::<PgResult<Vec<u8>>>()?;
            Ok(Some(bytes))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_payload_requires_a_typed_object_with_a_bound_candidate_digest() {
        let digest = "a".repeat(64);
        let valid = json!({"delivery_candidate_digest":digest,"passed":true,
            "output_digest":"b".repeat(64),"note":"kept","extra":[1,null]});
        validate_delivery_payload(&valid).unwrap();

        for (payload, invalid_field, constraint) in [
            (json!(null), "/args/delivery_payload", "type"),
            (json!("{}"), "/args/delivery_payload", "type"),
            (json!([]), "/args/delivery_payload", "type"),
            (json!(true), "/args/delivery_payload", "type"),
            (
                json!({}),
                "/args/delivery_payload/delivery_candidate_digest",
                "required",
            ),
            (
                json!({"delivery_candidate_digest":null}),
                "/args/delivery_payload/delivery_candidate_digest",
                "pattern",
            ),
            (
                json!({"delivery_candidate_digest":"A".repeat(64)}),
                "/args/delivery_payload/delivery_candidate_digest",
                "pattern",
            ),
            (
                json!({"delivery_candidate_digest":"a".repeat(63)}),
                "/args/delivery_payload/delivery_candidate_digest",
                "pattern",
            ),
            (
                json!({"delivery_candidate_digest":digest,"passed":"yes"}),
                "/args/delivery_payload/passed",
                "type",
            ),
            (
                json!({"delivery_candidate_digest":digest,"output_digest":7}),
                "/args/delivery_payload/output_digest",
                "type",
            ),
            (
                json!({"delivery_candidate_digest":digest,"output_digest":"g".repeat(64)}),
                "/args/delivery_payload/output_digest",
                "pattern",
            ),
        ] {
            let error = validate_delivery_payload(&payload).err().unwrap();
            assert!(error.is_delivery_payload_invalid(), "{payload}");
            let diagnostic = error.delivery_payload_invalid_diagnostic().unwrap();
            assert_eq!(diagnostic["reason"], "delivery_payload_invalid");
            assert_eq!(diagnostic["invalid_field"], invalid_field);
            assert_eq!(diagnostic["constraint"], constraint, "{payload}");
            // Caller values are never echoed; the response stays bounded.
            let serialized = diagnostic.to_string();
            assert!(!serialized.contains("kept"), "{serialized}");
            assert!(serialized.len() < 1024, "{serialized}");
        }
    }

    #[test]
    fn text_and_legacy_hex_preserve_exact_bytes() {
        for text in ["", "  report\r\n", "验证 🦀\n", "e\u{301}", "\0\t"] {
            let hex: String = text.as_bytes().iter().map(|b| format!("{b:02X}")).collect();
            let plain = decode(None, Some(text)).unwrap().unwrap();
            let legacy = decode(Some(&hex), None).unwrap().unwrap();
            assert_eq!(plain, text.as_bytes());
            assert_eq!(legacy, plain);
            assert_eq!(sha256_hex(&legacy), sha256_hex(&plain));
        }
        assert_eq!(
            decode(Some("00Ff80"), None).unwrap(),
            Some(vec![0, 255, 128])
        );
        assert_eq!(decode(None, None).unwrap(), None);
        assert!(decode(Some(""), Some("")).is_err());
        assert!(decode(Some("61"), Some("a")).is_err());
    }

    #[test]
    fn byte_limits_and_malformed_hex_are_enforced() {
        for hex in ["f", "gg", "ｆｆ", "61\n", "0x61"] {
            assert!(decode(Some(hex), None).is_err(), "invalid hex: {hex:?}");
        }
        let mut text = "🦀".repeat(MAX_BYTES / 4);
        assert_eq!(decode(None, Some(&text)).unwrap().unwrap().len(), MAX_BYTES);
        text.push('a');
        assert!(text.chars().count() < MAX_BYTES);
        assert!(decode(None, Some(&text)).is_err());
        let mut hex = "ff".repeat(MAX_BYTES);
        assert_eq!(decode(Some(&hex), None).unwrap().unwrap().len(), MAX_BYTES);
        hex.push_str("00");
        assert!(decode(Some(&hex), None).is_err());
    }

    #[test]
    fn generic_payload_and_optional_artifact_compatibility() {
        for payload in [
            Value::Null,
            json!("note"),
            json!([1, "note"]),
            json!({"note":1}),
        ] {
            let mut args = json!({"session_id":"s","expected_session_version":"1",
                "dirty_tree":false,"payload":payload});
            let evidence: SubmitEvidence = serde_json::from_value(args.clone()).unwrap();
            assert_eq!(evidence.payload, payload);
            assert!(evidence.artifact_text.is_none());
            assert!(evidence.artifact_hex.is_none());
            args.as_object_mut().unwrap().remove("dirty_tree");
            assert!(serde_json::from_value::<SubmitEvidence>(args).is_err());
        }
    }

    #[test]
    fn explicit_null_encoding_is_absent_in_the_command_parser() {
        for (hex, text, expected) in [
            (json!("61"), Value::Null, Some(b"a".to_vec())),
            (Value::Null, json!("a"), Some(b"a".to_vec())),
            (Value::Null, Value::Null, None),
        ] {
            let evidence: SubmitEvidence = serde_json::from_value(json!({
                "session_id":"s","expected_session_version":"1","dirty_tree":false,
                "payload":null,"artifact_hex":hex,"artifact_text":text,
            }))
            .unwrap();
            assert_eq!(
                decode(
                    evidence.artifact_hex.as_deref(),
                    evidence.artifact_text.as_deref()
                )
                .unwrap(),
                expected
            );
        }
    }
}
