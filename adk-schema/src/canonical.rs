use crate::document::JsonSchemaDialect;
use crate::error::{LimitKind, Result, SchemaError};
use serde_json::Value;
use std::io::{Error as IoError, ErrorKind, Result as IoResult, Write};

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self { bytes: Vec::new(), limit }
    }
    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        if self.bytes.len() + buf.len() > self.limit {
            return Err(IoError::new(ErrorKind::OutOfMemory, "limit exceeded"));
        }
        self.bytes.write(buf)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.bytes.flush()
    }
}

pub(crate) fn serialize_bounded(value: &Value, limit: usize) -> Result<Vec<u8>> {
    let mut writer = LimitedWriter::new(limit);
    serde_json::to_writer(&mut writer, value).map_err(|e| {
        if e.is_io() {
            SchemaError::LimitExceeded {
                kind: LimitKind::CanonicalBytes,
                limit,
                observed: limit + 1,
                pointer: String::new(),
            }
        } else {
            SchemaError::Serialization(e.to_string())
        }
    })?;
    Ok(writer.into_inner())
}

pub(crate) fn canonicalize(value: Value, expected: JsonSchemaDialect) -> Result<Value> {
    let mut val = value;
    if let Some(obj) = val.as_object_mut()
        && let Some(schema_val) = obj.get("$schema")
    {
        let schema_str = schema_val.as_str().ok_or_else(|| SchemaError::Parse {
            message: "$schema must be a string".to_string(),
        })?;
        let expected_uri = match expected {
            JsonSchemaDialect::Draft202012 => "https://json-schema.org/draft/2020-12/schema",
        };
        if schema_str != expected_uri {
            return Err(SchemaError::DialectMismatch {
                declared: schema_str.to_string(),
                expected,
            });
        }
        obj.remove("$schema");
    }
    Ok(sort_value(val))
}

fn sort_value(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut sorted = serde_json::Map::with_capacity(map.len());
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            for (k, val) in entries {
                sorted.insert(k, sort_value(val));
            }
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sort_value).collect()),
        Value::Number(n) => Value::Number(canonical_number(n)),
        _ => v,
    }
}

/// Normalizes a float with no fractional part to its integer form.
///
/// The digest identifies a schema, so `{"maximum": 5}` and `{"maximum": 5.0}`
/// share one identity. Matches RFC 8785 (JCS) for integral floats, the only
/// divergence JSON Schema produces; `NaN` and infinities are not representable
/// in JSON.
fn canonical_number(n: serde_json::Number) -> serde_json::Number {
    let Some(float) = n.as_f64() else {
        return n;
    };
    if n.is_i64() || n.is_u64() || float.fract() != 0.0 {
        return n;
    }

    // JCS renders `-0.0` as `0`.
    //
    // The bound is strict. `u64::MAX as f64` rounds *up* to 2^64, so a
    // non-strict `<=` would admit 2^64 itself, and `as u64` saturates it back
    // down to `u64::MAX` — giving 2^64 and 2^64 - 1 one shared identity. Every
    // integral f64 strictly below 2^64 is exactly representable as a `u64`.
    if float >= 0.0 && float < u64::MAX as f64 {
        #[expect(clippy::cast_possible_truncation, reason = "fract() == 0 and range-checked")]
        #[expect(clippy::cast_sign_loss, reason = "guarded by float >= 0.0")]
        return serde_json::Number::from(float as u64);
    }
    if float >= i64::MIN as f64 && float < 0.0 {
        #[expect(clippy::cast_possible_truncation, reason = "fract() == 0 and range-checked")]
        return serde_json::Number::from(float as i64);
    }
    n
}

#[cfg(test)]
mod tests {
    use crate::{IngestionPolicy, InputSchema};
    use serde_json::json;

    fn digest_of(schema: serde_json::Value) -> crate::SchemaDigest {
        InputSchema::from_value(schema, &IngestionPolicy::default())
            .expect("fixture schema ingests")
            .digest()
    }

    mod canonicalize {
        use super::*;

        /// `5.0` and `5` are the same constraint, so they share one identity.
        /// Schemas round-tripping through a single-numeric-type language emit
        /// either form.
        #[test]
        fn integral_floats_and_integers_share_a_digest() {
            assert_eq!(
                digest_of(json!({ "maximum": 5.0 })),
                digest_of(json!({ "maximum": 5 })),
                "`5.0` and `5` are the same constraint and must share an identity",
            );
        }

        /// Fractional values are distinct constraints.
        #[test]
        fn fractional_numbers_keep_their_own_digest() {
            assert_ne!(
                digest_of(json!({ "maximum": 5.5 })),
                digest_of(json!({ "maximum": 5 })),
                "`5.5` and `5` are different constraints",
            );
        }

        /// JCS renders negative zero as `0`.
        #[test]
        fn negative_zero_normalizes_to_zero() {
            assert_eq!(digest_of(json!({ "minimum": -0.0 })), digest_of(json!({ "minimum": 0 })),);
        }

        /// Normalization reaches nested and array positions.
        #[test]
        fn normalization_reaches_nested_values() {
            assert_eq!(
                digest_of(json!({ "properties": { "n": { "enum": [1.0, 2.0] } } })),
                digest_of(json!({ "properties": { "n": { "enum": [1, 2] } } })),
            );
        }
    }

    /// `u64::MAX as f64` rounds up to 2^64, so a non-strict bound would admit
    /// 2^64 and saturate it back to `u64::MAX`, giving two distinct constraints
    /// one identity.
    #[test]
    fn two_to_the_64_does_not_canonicalize_to_u64_max() {
        let at_bound = InputSchema::from_value(
            serde_json::json!({ "maximum": 18446744073709551616.0_f64 }),
            &IngestionPolicy::default(),
        )
        .expect("ingests");
        let max = InputSchema::from_value(
            serde_json::json!({ "maximum": u64::MAX }),
            &IngestionPolicy::default(),
        )
        .expect("ingests");

        assert_ne!(at_bound.digest(), max.digest(), "2^64 and u64::MAX share a digest");
    }
}
