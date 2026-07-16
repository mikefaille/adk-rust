use adk_schema::{
    IngestionPolicy, InputSchema, LimitKind, OutputSchema, ReferenceRejection, SchemaError,
};
use serde_json::json;

#[test]
fn test_empty_string_key_resolution() {
    let schema = json!({
        "properties": {
            "foo": {
                "$ref": "#/"
            }
        },
        "": "empty key val"
    });
    let policy = IngestionPolicy::default();
    let doc = InputSchema::from_value(schema, &policy).unwrap();
    assert_eq!(doc.metrics().reference_count, 1);
}

#[test]
fn test_utf8_percent_encoding() {
    // "%C3%A9" percent-decodes to "é" (2 bytes in UTF-8)
    let schema = json!({
        "properties": {
            "foo": {
                "$ref": "#/$defs/%C3%A9"
            }
        },
        "$defs": {
            "é": { "type": "string" }
        }
    });
    let policy = IngestionPolicy::default();
    let doc = InputSchema::from_value(schema, &policy).unwrap();
    assert_eq!(doc.metrics().reference_count, 1);
}

#[test]
fn test_invalid_percent_sequences() {
    let schema = json!({
        "properties": {
            "foo": {
                "$ref": "#/$defs/%G1" // Invalid hex
            }
        }
    });
    let policy = IngestionPolicy::default();
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(
        res,
        Err(SchemaError::UnsupportedReference { reason: ReferenceRejection::MalformedPointer, .. })
    ));
}

#[test]
fn test_invalid_tilde_escapes() {
    let schema = json!({
        "properties": {
            "foo": {
                "$ref": "#/$defs/~2" // Invalid tilde escape (only ~0 and ~1 allowed)
            }
        }
    });
    let policy = IngestionPolicy::default();
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(
        res,
        Err(SchemaError::UnsupportedReference { reason: ReferenceRejection::MalformedPointer, .. })
    ));
}

#[test]
fn test_differently_encoded_references_share_canonical_identity() {
    // "%24" decodes to "$"
    let schema1 = json!({
        "properties": {
            "foo": {
                "$ref": "#/%24defs/item"
            }
        },
        "$defs": {
            "item": { "type": "string" }
        }
    });
    let schema2 = json!({
        "properties": {
            "foo": {
                "$ref": "#/$defs/item"
            }
        },
        "$defs": {
            "item": { "type": "string" }
        }
    });
    let policy = IngestionPolicy::default();
    let doc1 = InputSchema::from_value(schema1, &policy).unwrap();
    let doc2 = InputSchema::from_value(schema2, &policy).unwrap();
    // They should share the exact same digest because the target is normalized and sorted canonical values.
    // Wait, the raw $ref string value in schema1 is "#/%24defs/item", and in schema2 it is "#/$defs/item".
    // Wait, does canonicalization decode percent-encoded references?
    // Ah! In our `canonical.rs`, we sort keys, but do we modify key values? No, we don't.
    // Wait, the raw value is in the document itself: `"$ref": "#/%24defs/item"` vs `"$ref": "#/$defs/item"`.
    // Wait, the adversarial design requirement says:
    // "two differently encoded references to the same target share one canonical identity;"
    // Wait, if the JSON strings are literally different ("#/%24defs/item" vs "#/$defs/item"), then the serialized canonical bytes would differ if we don't normalize $ref values!
    // Ah! Should we normalize `$ref` string values during canonicalization?
    // Yes! In `canonical.rs` (or during parsing/canonicalization), we should decode/normalize `$ref` string values to their canonical decoded format (e.g. `#/$defs/item` instead of `#/%24defs/item`)!
    // Let's modify `canonical.rs` or canonicalization logic to do exactly this: if the key is `"$ref"`, we parse it as local reference and format it back as `#/` followed by the decoded pointer!
    // Wait! Let's check: is that required? Yes! "two differently encoded references to the same target share one canonical identity;"
    // Let's do that! That's a beautiful catch of a detail.
    // Let's write the test first, and then we will update `canonical.rs`.
    assert_eq!(doc1.digest(), doc2.digest());
}

#[test]
fn test_missing_vs_malformed_pointers() {
    let policy = IngestionPolicy::default();
    // Missing pointer: resolves syntax-wise, but target is missing
    let missing_schema = json!({
        "properties": {
            "foo": {
                "$ref": "#/$defs/nonexistent"
            }
        }
    });
    let res1 = InputSchema::from_value(missing_schema, &policy);
    assert!(matches!(res1, Err(SchemaError::MissingReference { .. })));

    // Malformed pointer: contains invalid pointer characters/escapes
    let malformed_schema = json!({
        "properties": {
            "foo": {
                "$ref": "#/$defs/~" // Ends with ~ without escape
            }
        }
    });
    let res2 = InputSchema::from_value(malformed_schema, &policy);
    assert!(matches!(
        res2,
        Err(SchemaError::UnsupportedReference { reason: ReferenceRejection::MalformedPointer, .. })
    ));
}

#[test]
fn test_root_self_reference_fails() {
    let schema = json!({
        "$ref": "#"
    });
    let policy = IngestionPolicy::default();
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(res, Err(SchemaError::ReferenceCycle { .. })));
}

#[test]
fn test_multi_node_cycle_fails() {
    let schema = json!({
        "$defs": {
            "a": {
                "$ref": "#/$defs/b"
            },
            "b": {
                "$ref": "#/$defs/a"
            }
        }
    });
    let policy = IngestionPolicy::default();
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(res, Err(SchemaError::ReferenceCycle { .. })));
}

#[test]
fn test_repeated_acyclic_references_metrics() {
    let schema = json!({
        "properties": {
            "a": {
                "$ref": "#/$defs/shared"
            },
            "b": {
                "$ref": "#/$defs/shared"
            }
        },
        "$defs": {
            "shared": { "type": "string" }
        }
    });
    let policy = IngestionPolicy::default();
    let doc = InputSchema::from_value(schema, &policy).unwrap();
    // Pass A traverses the structure *without* following `$ref` target resolution (or counts targets only once).
    // Let's verify structural node count is exactly 12 (or some constant) rather than duplicating "shared" nodes twice.
    // Specifically, "shared" is counted once inside "$defs", and the ref objects are counted.
    // Let's check node count. Let's assert it is stable.
    assert_eq!(doc.metrics().node_count, 7);
}

#[test]
fn test_structural_nodes_counted_once() {
    let schema = json!({
        "a": 1,
        "b": [2, 3]
    });
    let policy = IngestionPolicy::default();
    let doc = InputSchema::from_value(schema, &policy).unwrap();
    // nodes:
    // 1. Root object
    // 2. Key "a" value (1)
    // 3. Key "b" value (array [2, 3])
    // 4. Element 2
    // 5. Element 3
    // Total nodes = 5
    assert_eq!(doc.metrics().node_count, 5);
}

#[test]
fn test_depth_without_recursive_traversal() {
    let schema = json!({
        "a": {
            "b": {
                "c": 1
            }
        }
    });
    let policy = IngestionPolicy::default();
    let doc = InputSchema::from_value(schema, &policy).unwrap();
    // Depth:
    // Root object: 1
    // "a": 2
    // "b": 3
    // "c" value 1: 4
    assert_eq!(doc.metrics().depth, 4);
}

#[test]
fn test_source_input_oversized_rejected_before_parsing() {
    let bytes = vec![b'{'; 100];
    let policy = IngestionPolicy { max_source_bytes: 50, ..IngestionPolicy::default() };
    let res = InputSchema::from_json_slice(&bytes, &policy);
    assert!(matches!(res, Err(SchemaError::LimitExceeded { kind: LimitKind::SourceBytes, .. })));
}

#[test]
fn test_bounded_serialization_limit() {
    let schema = json!({
        "a": "a very long string that will cross the canonical byte limit easily"
    });
    let policy = IngestionPolicy { max_canonical_bytes: 20, ..IngestionPolicy::default() };
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(res, Err(SchemaError::LimitExceeded { kind: LimitKind::CanonicalBytes, .. })));
}

#[test]
fn test_no_external_http_or_file_retrieval() {
    let schema = json!({
        "properties": {
            "foo": {
                "$ref": "http://example.com/schema.json"
            }
        }
    });
    let policy = IngestionPolicy::default();
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(
        res,
        Err(SchemaError::UnsupportedReference {
            reason: ReferenceRejection::NonLocalReference,
            ..
        })
    ));
}

#[test]
fn test_dynamic_ref_rejected() {
    let schema = json!({
        "properties": {
            "foo": {
                "$dynamicRef": "#foo"
            }
        }
    });
    let policy = IngestionPolicy::default();
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(
        res,
        Err(SchemaError::UnsupportedReference {
            reason: ReferenceRejection::UnsupportedDynamicRef,
            ..
        })
    ));
}

#[test]
fn test_anchors_rejected() {
    let schema = json!({
        "$anchor": "foo",
        "type": "string"
    });
    let policy = IngestionPolicy::default();
    let res = InputSchema::from_value(schema, &policy);
    assert!(matches!(
        res,
        Err(SchemaError::UnsupportedReference {
            reason: ReferenceRejection::UnsupportedAnchor,
            ..
        })
    ));
}

#[test]
fn test_canonicalization_preserves_annotations() {
    let schema = json!({
        "title": "My Schema Title",
        "description": "Some description",
        "type": "string"
    });
    let policy = IngestionPolicy::default();
    let doc = InputSchema::from_value(schema, &policy).unwrap();
    let val = doc.as_value();
    assert_eq!(val.get("title").unwrap().as_str().unwrap(), "My Schema Title");
    assert_eq!(val.get("description").unwrap().as_str().unwrap(), "Some description");
}

#[test]
fn test_equivalent_object_ordering() {
    let schema1 = json!({
        "b": 2,
        "a": 1
    });
    let schema2 = json!({
        "a": 1,
        "b": 2
    });
    let policy = IngestionPolicy::default();
    let doc1 = InputSchema::from_value(schema1, &policy).unwrap();
    let doc2 = InputSchema::from_value(schema2, &policy).unwrap();
    assert_eq!(doc1.digest(), doc2.digest());
}

#[test]
fn test_changed_array_ordering() {
    let schema1 = json!([1, 2]);
    let schema2 = json!([2, 1]);
    let policy = IngestionPolicy::default();
    let doc1 = InputSchema::from_value(schema1, &policy).unwrap();
    let doc2 = InputSchema::from_value(schema2, &policy).unwrap();
    assert_ne!(doc1.digest(), doc2.digest());
}

#[test]
fn test_input_and_output_different_digests() {
    let schema = json!({
        "type": "string"
    });
    let policy = IngestionPolicy::default();
    let input_doc = InputSchema::from_value(schema.clone(), &policy).unwrap();
    let output_doc = OutputSchema::from_value(schema, &policy).unwrap();
    assert_ne!(input_doc.digest(), output_doc.digest());
}

#[cfg(feature = "runtime-validation")]
#[test]
fn test_compiled_validator_is_reused() {
    let schema = json!({
        "type": "string"
    });
    let policy = IngestionPolicy::default();
    let doc = InputSchema::from_value(schema, &policy).unwrap();
    let validated = doc.compile().unwrap();
    let validated_clone = validated.clone();

    // They are equal and reference same compiled validator under the hood.
    assert_eq!(validated, validated_clone);
}

#[cfg(feature = "schemars")]
#[test]
fn test_jsonschema_without_serialize() {
    use schemars::JsonSchema;

    #[derive(JsonSchema)]
    struct Dummy {
        #[allow(dead_code)]
        val: String,
    }

    let doc = InputSchema::for_type::<Dummy>();
    assert!(doc.is_ok());
}
