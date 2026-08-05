use crate::document::SchemaDirection;
use crate::error::Result;
use crate::policy::IngestionPolicy;

mod sealed {
    pub trait Sealed {}
}

/// Marker trait for schema roles, sealed to prevent external implementation.
pub trait SchemaRole: sealed::Sealed + Send + Sync + 'static {
    /// The runtime direction associated with the role.
    const DIRECTION: SchemaDirection;
    /// The tag used during digest calculation.
    const DIGEST_TAG: u8;
}

/// Role for tool inputs.
#[derive(Debug)]
pub enum Input {}

/// Role for tool outputs.
#[derive(Debug)]
pub enum Output {}

impl sealed::Sealed for Input {}
impl sealed::Sealed for Output {}

impl SchemaRole for Input {
    const DIRECTION: SchemaDirection = SchemaDirection::Input;
    const DIGEST_TAG: u8 = 1;
}

impl SchemaRole for Output {
    const DIRECTION: SchemaDirection = SchemaDirection::Output;
    const DIGEST_TAG: u8 = 2;
}

/// Type alias for input schemas.
pub type InputSchema = crate::document::SchemaDocument<Input>;
/// Type alias for output schemas.
pub type OutputSchema = crate::document::SchemaDocument<Output>;

impl InputSchema {
    /// Merges static `allOf` subschemas (those containing static `properties` and `required` arrays
    /// without conditional `if`/`then` rules) into the top-level object schema.
    ///
    /// This produces an ideal, flattened schema representation at compile/init time, removing
    /// redundant nesting and simplifying runtime validation for Gemini and tools.
    pub fn flatten_static_all_of(&self, policy: &IngestionPolicy) -> Result<Self> {
        let mut val = self.as_value().clone();
        let Some(obj) = val.as_object_mut() else {
            return Ok(self.clone());
        };

        let Some(all_of_val) = obj.get("allOf").and_then(|v| v.as_array()).cloned() else {
            return Ok(self.clone());
        };

        let mut remaining_all_of = Vec::new();

        for item in all_of_val {
            let is_conditional = item.get("if").is_some()
                || item.get("then").is_some()
                || item.get("else").is_some();
            if is_conditional {
                remaining_all_of.push(item);
                continue;
            }

            if let Some(item_obj) = item.as_object() {
                // Merge properties
                if let Some(item_props) = item_obj.get("properties").and_then(|p| p.as_object()) {
                    let base_props = obj
                        .entry("properties")
                        .or_insert_with(|| serde_json::json!({}))
                        .as_object_mut();
                    if let Some(base_map) = base_props {
                        for (k, v) in item_props {
                            base_map.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }

                // Merge required fields
                if let Some(item_req) = item_obj.get("required").and_then(|r| r.as_array()) {
                    let base_req = obj
                        .entry("required")
                        .or_insert_with(|| serde_json::json!([]))
                        .as_array_mut();
                    if let Some(base_arr) = base_req {
                        for r_val in item_req {
                            if !base_arr.contains(r_val) {
                                base_arr.push(r_val.clone());
                            }
                        }
                    }
                }
            } else {
                remaining_all_of.push(item);
            }
        }

        if remaining_all_of.is_empty() {
            obj.remove("allOf");
        } else {
            obj.insert("allOf".to_string(), serde_json::Value::Array(remaining_all_of));
        }

        Self::from_value(val, policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flatten_static_all_of_merges_static_properties_and_required() {
        let policy = IngestionPolicy::default();
        let raw = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"],
            "allOf": [
                {
                    "properties": {
                        "age": { "type": "integer" }
                    },
                    "required": ["age"]
                }
            ]
        });

        let schema = InputSchema::from_value(raw, &policy).expect("valid schema");
        let flattened = schema.flatten_static_all_of(&policy).expect("flatten succeeds");
        let val = flattened.as_value();

        assert!(val.get("allOf").is_none(), "static allOf must be removed after flattening");
        assert!(val.pointer("/properties/name").is_some(), "base property must survive");
        assert!(val.pointer("/properties/age").is_some(), "static allOf property must be merged");
        let req = val.get("required").and_then(|r| r.as_array()).expect("required array exists");
        assert!(req.contains(&serde_json::json!("name")));
        assert!(req.contains(&serde_json::json!("age")));
    }
}
