use crate::document::SchemaDocument;
use crate::pointer::child;
use crate::role::SchemaRole;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// One property a schema declares.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldEntry {
    /// Property key, unescaped.
    pub name: String,
    /// RFC 6901 pointer to this property's definition.
    pub pointer: String,
    /// Declared types, exactly as written.
    ///
    /// A union stays a union: `["string", "null"]` is reported as two entries,
    /// not collapsed into a nullability flag, because the collapse cannot be
    /// reversed and `anyOf` expresses the same thing differently.
    pub types: Vec<String>,
    /// Whether an unconditional `required` names this property.
    ///
    /// Conditional requirements are never counted here — see
    /// [`SchemaDocument::fields`].
    pub required: bool,
    /// Vendor `x-` keywords, uninterpreted.
    pub extensions: Map<String, Value>,
}

impl<R: SchemaRole> SchemaDocument<R> {
    /// A flat inventory of the properties this schema declares.
    ///
    /// For consumers with no typed model of their own — dynamic UIs, logging,
    /// generic policy engines — that would otherwise walk the schema tree by
    /// hand. A consumer that already has typed domain objects should read those
    /// instead of recovering facts from a projection.
    ///
    /// Merges unconditional `allOf` branches best-effort. **Conditional
    /// branches are left alone**: an `allOf` entry carrying `if`, `then`, or
    /// `else` states an obligation that depends on a value, and promoting its
    /// `required` keys here would report them as unconditionally required —
    /// false for every instance that fails the condition. Use
    /// [`SchemaDocument::outstanding`] for conditional requirements.
    ///
    /// # Examples
    ///
    /// ```
    /// # use adk_schema::{IngestionPolicy, InputSchema, SchemaError};
    /// let schema = InputSchema::from_value(
    ///     serde_json::json!({
    ///         "properties": { "name": { "type": ["string", "null"] } },
    ///         "required": ["name"]
    ///     }),
    ///     &IngestionPolicy::default(),
    /// )?;
    ///
    /// let fields = schema.fields();
    ///
    /// assert_eq!(fields[0].name, "name");
    /// assert_eq!(fields[0].types, ["string", "null"]);
    /// assert!(fields[0].required);
    /// # Ok::<(), SchemaError>(())
    /// ```
    pub fn fields(&self) -> Vec<FieldEntry> {
        let schema = self.as_value();
        let mut collected: BTreeMap<String, FieldEntry> = BTreeMap::new();

        collect_properties(schema, "", &mut collected);

        for (index, branch) in
            schema.get("allOf").and_then(Value::as_array).into_iter().flatten().enumerate()
        {
            if is_conditional(branch) {
                continue;
            }
            collect_properties(branch, &format!("/allOf/{index}"), &mut collected);
        }

        collected.into_values().collect()
    }
}

/// Whether an `allOf` branch states a conditional obligation rather than a
/// plain intersection.
fn is_conditional(branch: &Value) -> bool {
    ["if", "then", "else"].iter().any(|keyword| branch.get(*keyword).is_some())
}

fn collect_properties(schema: &Value, prefix: &str, out: &mut BTreeMap<String, FieldEntry>) {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        // A branch may constrain `required` without redeclaring the property.
        for name in required {
            if let Some(entry) = out.get_mut(name) {
                entry.required = true;
            }
        }
        return;
    };

    let properties_pointer = child(prefix, "properties");

    for (name, definition) in properties {
        let is_required = required.contains(&name.as_str());
        out.entry(name.clone())
            .and_modify(|existing| existing.required |= is_required)
            .or_insert_with(|| FieldEntry {
                pointer: child(&properties_pointer, name),
                name: name.clone(),
                types: declared_types(definition),
                required: is_required,
                extensions: vendor_extensions(definition),
            });
    }
}

/// `type` is a string or an array of strings; both are reported as written.
fn declared_types(definition: &Value) -> Vec<String> {
    match definition.get("type") {
        Some(Value::String(single)) => vec![single.clone()],
        Some(Value::Array(many)) => {
            many.iter().filter_map(Value::as_str).map(str::to_string).collect()
        }
        _ => Vec::new(),
    }
}

fn vendor_extensions(definition: &Value) -> Map<String, Value> {
    definition
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(key, _)| key.starts_with("x-"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::{IngestionPolicy, InputSchema};
    use serde_json::{Value, json};

    fn schema(value: Value) -> InputSchema {
        InputSchema::from_value(value, &IngestionPolicy::default()).expect("fixture ingests")
    }

    mod fields {
        use super::*;

        #[test]
        fn reports_a_declared_property() {
            let fields = schema(json!({ "properties": { "name": { "type": "string" } } })).fields();

            assert_eq!(fields[0].name, "name");
        }

        #[test]
        fn marks_an_unconditionally_required_property() {
            let fields = schema(json!({
                "properties": { "name": {} },
                "required": ["name"]
            }))
            .fields();

            assert!(fields[0].required);
        }

        /// A union stays a union: collapsing it to a nullability flag cannot be
        /// reversed.
        #[test]
        fn preserves_a_type_union_as_written() {
            let fields =
                schema(json!({ "properties": { "n": { "type": ["string", "null"] } } })).fields();

            assert_eq!(fields[0].types, ["string", "null"]);
        }

        #[test]
        fn passes_vendor_extensions_through_uninterpreted() {
            let fields = schema(json!({
                "properties": { "n": { "x-caller-confirmation": "required" } }
            }))
            .fields();

            assert_eq!(fields[0].extensions["x-caller-confirmation"], "required");
        }

        /// The name is unescaped while the pointer is escaped — pointer syntax
        /// must not leak into a field name.
        #[test]
        fn reports_an_unescaped_name_for_a_property_containing_a_separator() {
            let fields = schema(json!({ "properties": { "a/b": {} } })).fields();

            assert_eq!(fields[0].name, "a/b");
        }

        #[test]
        fn reports_an_escaped_pointer_for_a_property_containing_a_separator() {
            let fields = schema(json!({ "properties": { "a/b": {} } })).fields();

            assert_eq!(fields[0].pointer, "/properties/a~1b");
        }
    }

    mod all_of {
        use super::*;

        #[test]
        fn merges_an_unconditional_branch() {
            let fields = schema(json!({
                "properties": { "a": {} },
                "allOf": [{ "properties": { "b": {} } }]
            }))
            .fields();

            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["a", "b"]);
        }

        #[test]
        fn honours_a_requirement_stated_in_an_unconditional_branch() {
            let fields = schema(json!({
                "properties": { "a": {} },
                "allOf": [{ "required": ["a"] }]
            }))
            .fields();

            assert!(fields[0].required);
        }

        /// The rule this primitive exists to respect. Promoting a conditional
        /// `then.required` would report `caller_name` as unconditionally
        /// required — false for every request that is not an order, and the
        /// exact bug `outstanding()` was built to fix.
        #[test]
        fn does_not_promote_a_conditional_requirement() {
            let fields = schema(json!({
                "properties": { "request_kind": {}, "caller_name": {} },
                "allOf": [{
                    "if": { "properties": { "request_kind": { "const": "order" } } },
                    "then": { "required": ["caller_name"] }
                }]
            }))
            .fields();

            let caller_name = fields.iter().find(|f| f.name == "caller_name").expect("declared");
            assert!(
                !caller_name.required,
                "a conditional obligation was reported as unconditional: {caller_name:?}",
            );
        }

        /// A conditional branch may also declare properties; those are not
        /// merged, so the flat view never gains a field that only exists under
        /// a condition.
        #[test]
        fn does_not_merge_properties_from_a_conditional_branch() {
            let fields = schema(json!({
                "properties": { "a": {} },
                "allOf": [{
                    "if": { "properties": { "a": { "const": 1 } } },
                    "then": { "properties": { "conditional_only": {} } }
                }]
            }))
            .fields();

            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["a"]);
        }
    }
}
