//! Gemini-specific schema normalization adapter.
//!
//! The [`GeminiSchemaAdapter`] applies all destructive transforms required by
//! Gemini's function-calling API. It composes shared utilities from
//! [`adk_core::schema_utils`] with Gemini-specific keyword removal to produce
//! schemas that Gemini accepts.

use adk_core::SchemaAdapter;
use adk_core::schema_adapter::{
    CompiledSchema, ProjectionDiagnostic, ProjectionPolicy, SchemaCompileError,
};
use adk_core::schema_utils;
use serde_json::{Map, Value};
use std::borrow::Cow;

/// Allowed `format` values for the Gemini API.
const GEMINI_ALLOWED_FORMATS: &[&str] =
    &["date-time", "date", "time", "email", "uri", "uuid", "int32", "int64", "float", "double"];

/// Keywords that Gemini does not support and must be removed from all schema nodes.
const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "$id",
    "additionalProperties",
    "contains",
    "contentEncoding",
    "contentMediaType",
    "default",
    "dependentRequired",
    "dependentSchemas",
    "deprecated",
    "examples",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minItems",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "not",
    "pattern",
    "patternProperties",
    "prefixItems",
    "propertyNames",
    "readOnly",
    "title",
    "unevaluatedProperties",
    "uniqueItems",
    "writeOnly",
];

const UNSUPPORTED_KEYWORDS_VERTEX: &[&str] = &[
    "$id",
    "contains",
    "contentEncoding",
    "contentMediaType",
    "default",
    "dependentRequired",
    "dependentSchemas",
    "deprecated",
    "examples",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minItems",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "not",
    "pattern",
    "patternProperties",
    "prefixItems",
    "propertyNames",
    "readOnly",
    "title",
    "unevaluatedProperties",
    "uniqueItems",
    "writeOnly",
];

/// Schema adapter for the Gemini API surface.
#[derive(Debug)]
pub struct GeminiSchemaAdapter {
    /// When `true`, targets the Vertex AI surface which requires
    /// `additionalProperties: false` on object schemas.
    vertex_ai: bool,
    /// Projection policy for this adapter.
    policy: ProjectionPolicy,
}

impl GeminiSchemaAdapter {
    /// Creates a new `GeminiSchemaAdapter` for the standard Gemini API surface.
    pub fn new() -> Self {
        Self { vertex_ai: false, policy: ProjectionPolicy::Exact }
    }

    /// Creates a new `GeminiSchemaAdapter` for the standard Gemini API surface with a specific policy.
    pub fn with_policy(policy: ProjectionPolicy) -> Self {
        Self { vertex_ai: false, policy }
    }

    /// Creates a new `GeminiSchemaAdapter` for the Vertex AI surface.
    pub fn vertex_ai() -> Self {
        Self { vertex_ai: true, policy: ProjectionPolicy::Exact }
    }

    /// Creates a new `GeminiSchemaAdapter` for the Vertex AI surface with a specific policy.
    pub fn vertex_ai_with_policy(policy: ProjectionPolicy) -> Self {
        Self { vertex_ai: true, policy }
    }
}

impl Default for GeminiSchemaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaAdapter for GeminiSchemaAdapter {
    fn identifier(&self) -> &str {
        "gemini"
    }

    fn version(&self) -> &str {
        "2.0.0"
    }

    fn surface(&self) -> Option<&str> {
        if self.vertex_ai { Some("vertex") } else { Some("studio") }
    }

    fn projection_policy(&self) -> ProjectionPolicy {
        self.policy
    }

    fn normalize_schema(&self, mut schema: Value) -> Value {
        // Fallback to legacy normalization for backward compatibility in non-compile paths.
        // In the future, this could be replaced by a call to compile_schema().value.
        let definitions = extract_definitions_legacy(&schema);
        schema_utils::resolve_refs(&mut schema, &definitions, 0);
        schema_utils::strip_schema_keyword(&mut schema);
        schema_utils::collapse_combiners(&mut schema);
        schema_utils::merge_all_of(&mut schema);
        schema_utils::collapse_type_arrays(&mut schema);
        schema_utils::strip_conditional_keywords(&mut schema);
        schema_utils::convert_const_to_enum(&mut schema);
        schema_utils::strip_null_from_enum(&mut schema);
        schema_utils::add_implicit_object_type(&mut schema);
        if self.vertex_ai {
            remove_unsupported_keywords_vertex_legacy(&mut schema);
        } else {
            remove_unsupported_keywords_legacy(&mut schema);
        }
        schema_utils::strip_unsupported_formats(&mut schema, GEMINI_ALLOWED_FORMATS);
        schema_utils::enforce_nesting_depth(&mut schema, 5, 0);
        if let Some(obj) = schema.as_object_mut() {
            obj.remove("definitions");
            obj.remove("$defs");
        }
        schema
    }

    fn compile_schema(&self, schema: &Value) -> Result<CompiledSchema, SchemaCompileError> {
        let mut compiler = GeminiCompiler::new(schema, self.policy, self.vertex_ai);
        let projected = compiler.compile(schema, "", 0)?;

        Ok(CompiledSchema {
            value: projected,
            target: self.identifier().to_string(),
            version: self.version().to_string(),
            surface: self.surface().map(|s| s.to_string()),
            policy: self.policy,
            diagnostics: compiler.diagnostics,
        })
    }

    fn validate_tool_name(&self, name: &str) -> Result<(), SchemaCompileError> {
        if name.len() > 64 {
            return Err(SchemaCompileError::new(format!(
                "tool name '{}' exceeds Gemini's 64-byte limit",
                name
            )));
        }
        Ok(())
    }

    fn normalize_tool_name<'a>(&self, name: &'a str) -> Cow<'a, str> {
        schema_utils::truncate_tool_name(name, 64)
    }

    fn empty_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
}

struct GeminiCompiler<'a> {
    root: &'a Value,
    policy: ProjectionPolicy,
    vertex_ai: bool,
    diagnostics: Vec<ProjectionDiagnostic>,
    ref_stack: Vec<String>,
}

impl<'a> GeminiCompiler<'a> {
    fn new(root: &'a Value, policy: ProjectionPolicy, vertex_ai: bool) -> Self {
        Self { root, policy, vertex_ai, diagnostics: Vec::new(), ref_stack: Vec::new() }
    }

    fn compile(
        &mut self,
        schema: &Value,
        path: &str,
        depth: usize,
    ) -> Result<Value, SchemaCompileError> {
        if depth > 5 {
            return self.error(path, "depth", "Schema exceeds Gemini's maximum nesting depth of 5");
        }

        match schema {
            Value::Object(obj) => self.compile_object(obj, path, depth),
            Value::Bool(b) => {
                if *b {
                    Ok(Value::Object(Map::new()))
                } else {
                    self.error(
                        path,
                        "boolean_schema",
                        "Gemini does not support 'false' schemas (not)",
                    )
                }
            }
            _ => Ok(schema.clone()),
        }
    }

    fn compile_object(
        &mut self,
        obj: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Result<Value, SchemaCompileError> {
        let mut projected = Map::new();

        // 1. Handle $ref (Truthful pointer resolution)
        if let Some(ref_val) = obj.get("$ref").and_then(|v| v.as_str()) {
            if self.ref_stack.contains(&ref_val.to_string()) {
                return self.error(
                    path,
                    "$ref",
                    format!("Recursive reference detected: {}", ref_val),
                );
            }

            let resolved = if ref_val == "#" {
                Some(self.root)
            } else if let Some(pointer) = ref_val.strip_prefix('#') {
                self.root.pointer(pointer)
            } else {
                None
            };

            match resolved {
                Some(resolved_schema) => {
                    self.ref_stack.push(ref_val.to_string());
                    let result = self.compile(resolved_schema, path, depth);
                    self.ref_stack.pop();
                    return result;
                }
                None => {
                    return self.error(
                        path,
                        "$ref",
                        format!("Unresolved or external $ref detected: '{}'", ref_val),
                    );
                }
            }
        }

        // 2. Handle allOf (Conflict detection)
        if let Some(all_of) = obj.get("allOf").and_then(|v| v.as_array()) {
            let mut merged = Map::new();
            for (i, sub) in all_of.iter().enumerate() {
                let sub_path = format!("{}/allOf/{}", path, i);
                let compiled_sub = self.compile(sub, &sub_path, depth)?;
                if let Some(sub_obj) = compiled_sub.as_object() {
                    for (k, v) in sub_obj {
                        if let Some(existing) = merged.get(k) {
                            if existing != v {
                                return self.error(
                                    path,
                                    "allOf",
                                    format!("Conflicting allOf branches for keyword '{}'", k),
                                );
                            }
                        }
                        merged.insert(k.clone(), v.clone());
                    }
                }
            }
            // Merge merged into the current object logic
            for (k, v) in merged {
                projected.insert(k, v);
            }
        }

        // 3. Handle anyOf/oneOf (Semantic loss)
        for keyword in &["anyOf", "oneOf"] {
            if let Some(arr) = obj.get(*keyword).and_then(|v| v.as_array()) {
                // Find first non-null
                let non_null = arr.iter().enumerate().find(|(_, s)| !is_null_schema(s));
                match non_null {
                    Some((i, sub)) => {
                        self.diagnostic(path, keyword, "Gemini does not support polymorphic unions; projecting to first non-null branch")?;
                        let sub_path = format!("{}/{}/{}", path, keyword, i);
                        let compiled_sub = self.compile(sub, &sub_path, depth)?;
                        if let Some(sub_obj) = compiled_sub.as_object() {
                            for (k, v) in sub_obj {
                                projected.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    None => {
                        if let Some(first) = arr.first() {
                            let compiled_sub = self.compile(first, path, depth)?;
                            if let Some(sub_obj) = compiled_sub.as_object() {
                                for (k, v) in sub_obj {
                                    projected.insert(k.clone(), v.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. Handle Type (Type arrays semantic loss)
        if let Some(type_val) = obj.get("type") {
            if let Some(arr) = type_val.as_array() {
                let non_null = arr.iter().find(|v| v.as_str() != Some("null"));
                match non_null {
                    Some(t) => {
                        self.diagnostic(path, "type", "Gemini does not support type arrays; projecting to first non-null type")?;
                        projected.insert("type".to_string(), t.clone());
                    }
                    None => {
                        if let Some(first) = arr.first() {
                            projected.insert("type".to_string(), first.clone());
                        }
                    }
                }
            } else {
                projected.insert("type".to_string(), type_val.clone());
            }
        }

        // 5. Audit destructive transforms
        for (key, value) in obj {
            if key == "$ref"
                || key == "allOf"
                || key == "anyOf"
                || key == "oneOf"
                || key == "type"
                || key == "definitions"
                || key == "$defs"
                || key == "$schema"
            {
                continue;
            }

            match key.as_str() {
                "properties" => {
                    let mut compiled_props = Map::new();
                    if let Some(props_obj) = value.as_object() {
                        for (pk, pv) in props_obj {
                            let prop_path = format!("{}/properties/{}", path, pk);
                            compiled_props
                                .insert(pk.clone(), self.compile(pv, &prop_path, depth + 1)?);
                        }
                    }
                    projected.insert("properties".to_string(), Value::Object(compiled_props));
                    if !projected.contains_key("type") {
                        projected.insert("type".to_string(), Value::String("object".to_string()));
                    }
                }
                "items" => {
                    if let Some(items_arr) = value.as_array() {
                        self.diagnostic(
                            path,
                            "items",
                            "Gemini does not support tuple validation; projecting to first schema",
                        )?;
                        if let Some(first) = items_arr.first() {
                            let items_path = format!("{}/items/0", path);
                            projected.insert(
                                "items".to_string(),
                                self.compile(first, &items_path, depth + 1)?,
                            );
                        }
                    } else {
                        let items_path = format!("{}/items", path);
                        projected.insert(
                            "items".to_string(),
                            self.compile(value, &items_path, depth + 1)?,
                        );
                    }
                }
                "enum" => {
                    if let Some(arr) = value.as_array() {
                        let filtered: Vec<Value> =
                            arr.iter().filter(|v| !v.is_null()).cloned().collect();
                        if filtered.len() != arr.len() {
                            self.diagnostic(
                                path,
                                "enum",
                                "Gemini does not support null in enum; stripping null values",
                            )?;
                        }
                        if !filtered.is_empty() {
                            projected.insert("enum".to_string(), Value::Array(filtered));
                        }
                    }
                }
                "const" => {
                    projected.insert("enum".to_string(), Value::Array(vec![value.clone()]));
                }
                "description" => {
                    projected.insert("description".to_string(), value.clone());
                }
                "nullable" => {
                    projected.insert("nullable".to_string(), value.clone());
                }
                "format" => {
                    if let Some(f) = value.as_str() {
                        if GEMINI_ALLOWED_FORMATS.contains(&f) {
                            projected.insert("format".to_string(), value.clone());
                        } else {
                            self.diagnostic(
                                path,
                                "format",
                                format!("Gemini does not support format '{}'; stripping", f),
                            )?;
                        }
                    }
                }
                "additionalProperties" => {
                    if self.vertex_ai {
                        projected.insert("additionalProperties".to_string(), Value::Bool(false));
                        if value != &Value::Bool(false) {
                            self.diagnostic(
                                path,
                                "additionalProperties",
                                "Vertex AI requires additionalProperties: false",
                            )?;
                        }
                    } else {
                        self.diagnostic(
                            path,
                            "additionalProperties",
                            "Gemini Studio does not support additionalProperties; stripping",
                        )?;
                    }
                }
                "required" => {
                    projected.insert("required".to_string(), value.clone());
                }
                // Destructive transforms for validation keywords
                "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "minLength"
                | "maxLength" | "pattern" | "minItems" | "maxItems" | "uniqueItems"
                | "minProperties" | "maxProperties" | "multipleOf" => {
                    self.diagnostic(
                        path,
                        key,
                        format!("Gemini does not support validation keyword '{}'; stripping", key),
                    )?;
                }
                // Conditional and other complex keywords
                "if"
                | "then"
                | "else"
                | "dependentRequired"
                | "dependentSchemas"
                | "contains"
                | "not"
                | "patternProperties"
                | "propertyNames"
                | "unevaluatedProperties"
                | "prefixItems" => {
                    self.diagnostic(
                        path,
                        key,
                        format!("Gemini does not support complex keyword '{}'; stripping", key),
                    )?;
                }
                _ => {
                    // Ignore unknown keywords or strip them? The request says audit every destructive transform.
                }
            }
        }

        if self.vertex_ai && !projected.contains_key("additionalProperties") {
            if projected.get("type").and_then(|v| v.as_str()) == Some("object") {
                projected.insert("additionalProperties".to_string(), Value::Bool(false));
            }
        }

        Ok(Value::Object(projected))
    }

    fn diagnostic(
        &mut self,
        path: &str,
        keyword: &str,
        message: impl Into<String>,
    ) -> Result<(), SchemaCompileError> {
        let msg = message.into();
        if self.policy == ProjectionPolicy::Exact {
            return Err(SchemaCompileError::new(format!("Semantic loss at {}: {}", path, msg)));
        }
        self.diagnostics.push(ProjectionDiagnostic {
            path: path.to_string(),
            keyword: keyword.to_string(),
            message: msg,
        });
        Ok(())
    }

    fn error<T>(
        &mut self,
        path: &str,
        keyword: &str,
        message: impl Into<String>,
    ) -> Result<T, SchemaCompileError> {
        Err(SchemaCompileError::new(format!(
            "Compilation error at {} ({}): {}",
            path,
            keyword,
            message.into()
        )))
    }
}

fn is_null_schema(schema: &Value) -> bool {
    schema
        .as_object()
        .and_then(|obj| obj.get("type"))
        .and_then(|t| t.as_str())
        .is_some_and(|t| t == "null")
}

fn extract_definitions_legacy(schema: &Value) -> Map<String, Value> {
    let mut defs = Map::new();
    if let Some(obj) = schema.as_object() {
        if let Some(definitions) = obj.get("definitions").and_then(|v| v.as_object()) {
            for (key, value) in definitions {
                defs.insert(key.clone(), value.clone());
            }
        }
        if let Some(dollar_defs) = obj.get("$defs").and_then(|v| v.as_object()) {
            for (key, value) in dollar_defs {
                defs.insert(key.clone(), value.clone());
            }
        }
    }
    defs
}

fn remove_unsupported_keywords_legacy(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    for keyword in UNSUPPORTED_KEYWORDS {
        obj.remove(*keyword);
    }
    let is_array_type = obj.get("type").and_then(|t| t.as_str()).is_some_and(|t| t == "array");
    if !is_array_type {
        obj.remove("items");
    } else if obj.get("items").is_some_and(|v| v.is_array()) {
        let first_schema = obj
            .get("items")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"type": "string"}));
        obj.insert("items".to_string(), first_schema);
    } else if !obj.contains_key("items") {
        obj.insert("items".to_string(), serde_json::json!({"type": "string"}));
    }
    if let Some(props) = obj.get_mut("properties").and_then(|v| v.as_object_mut()) {
        for value in props.values_mut() {
            remove_unsupported_keywords_legacy(value);
        }
    }
    if let Some(items) = obj.get_mut("items") {
        if items.is_object() {
            remove_unsupported_keywords_legacy(items);
        }
    }
    for keyword in &["allOf", "anyOf", "oneOf"] {
        if let Some(arr) = obj.get_mut(*keyword).and_then(|v| v.as_array_mut()) {
            for sub in arr {
                remove_unsupported_keywords_legacy(sub);
            }
        }
    }
}

fn remove_unsupported_keywords_vertex_legacy(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    for keyword in UNSUPPORTED_KEYWORDS_VERTEX {
        obj.remove(*keyword);
    }
    let is_object_type = obj.get("type").and_then(|t| t.as_str()).is_some_and(|t| t == "object");
    if is_object_type {
        obj.insert("additionalProperties".to_string(), Value::Bool(false));
    } else {
        obj.remove("additionalProperties");
    }
    let is_array_type = obj.get("type").and_then(|t| t.as_str()).is_some_and(|t| t == "array");
    if !is_array_type {
        obj.remove("items");
    } else if obj.get("items").is_some_and(|v| v.is_array()) {
        let first_schema = obj
            .get("items")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"type": "string"}));
        obj.insert("items".to_string(), first_schema);
    } else if !obj.contains_key("items") {
        obj.insert("items".to_string(), serde_json::json!({"type": "string"}));
    }
    if let Some(props) = obj.get_mut("properties").and_then(|v| v.as_object_mut()) {
        for value in props.values_mut() {
            remove_unsupported_keywords_vertex_legacy(value);
        }
    }
    if let Some(items) = obj.get_mut("items") {
        if items.is_object() {
            remove_unsupported_keywords_vertex_legacy(items);
        }
    }
    for keyword in &["allOf", "anyOf", "oneOf"] {
        if let Some(arr) = obj.get_mut(*keyword).and_then(|v| v.as_array_mut()) {
            for sub in arr {
                remove_unsupported_keywords_vertex_legacy(sub);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compile_schema_pointer_resolution() {
        let adapter = GeminiSchemaAdapter::new();
        let schema = json!({
            "type": "object",
            "properties": {
                "foo": { "$ref": "#/definitions/Foo" },
                "bar": { "$ref": "#/$defs/Bar" }
            },
            "definitions": {
                "Foo": { "type": "string" }
            },
            "$defs": {
                "Bar": { "type": "number" }
            }
        });
        let result = adapter.compile_schema(&schema).unwrap();
        assert_eq!(result.value["properties"]["foo"]["type"], "string");
        assert_eq!(result.value["properties"]["bar"]["type"], "number");
    }

    #[test]
    fn test_compile_schema_recursive_detection() {
        let adapter = GeminiSchemaAdapter::new();
        let schema = json!({
            "definitions": {
                "Node": {
                    "type": "object",
                    "properties": {
                        "child": { "$ref": "#/definitions/Node" }
                    }
                }
            },
            "properties": {
                "root": { "$ref": "#/definitions/Node" }
            }
        });
        let result = adapter.compile_schema(&schema);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Recursive reference detected"));
    }

    #[test]
    fn test_compile_schema_semantic_loss_exact() {
        let adapter = GeminiSchemaAdapter::new(); // Default is Exact
        let schema = json!({
            "type": "string",
            "pattern": "^[a-z]+$"
        });
        let result = adapter.compile_schema(&schema);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("Gemini does not support validation keyword 'pattern'")
        );
    }

    #[test]
    fn test_compile_schema_semantic_loss_runtime_validated() {
        let adapter = GeminiSchemaAdapter::with_policy(ProjectionPolicy::RuntimeValidated);
        let schema = json!({
            "type": "string",
            "pattern": "^[a-z]+$"
        });
        let result = adapter.compile_schema(&schema).unwrap();
        assert_eq!(result.value["type"], "string");
        assert!(result.value.get("pattern").is_none());
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].keyword, "pattern");
    }

    #[test]
    fn test_compile_schema_vertex_additional_properties() {
        let adapter = GeminiSchemaAdapter::vertex_ai();
        let schema = json!({
            "type": "object",
            "properties": { "x": { "type": "string" } }
        });
        let result = adapter.compile_schema(&schema).unwrap();
        assert_eq!(result.value["additionalProperties"], false);
    }

    #[test]
    fn test_compile_schema_studio_additional_properties() {
        let adapter = GeminiSchemaAdapter::new();
        let schema = json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "additionalProperties": true
        });
        // Exact policy will fail on additionalProperties stripping
        let result = adapter.compile_schema(&schema);
        assert!(result.is_err());

        let adapter_rv = GeminiSchemaAdapter::with_policy(ProjectionPolicy::RuntimeValidated);
        let result_rv = adapter_rv.compile_schema(&schema).unwrap();
        assert!(result_rv.value.get("additionalProperties").is_none());
        assert!(result_rv.diagnostics.iter().any(|d| d.keyword == "additionalProperties"));
    }

    #[test]
    fn test_compile_schema_distinct_definition_paths() {
        let adapter = GeminiSchemaAdapter::new();
        let schema = json!({
            "type": "object",
            "properties": {
                "foo": { "$ref": "#/definitions/Shared" },
                "bar": { "$ref": "#/$defs/Shared" }
            },
            "definitions": {
                "Shared": { "type": "string" }
            },
            "$defs": {
                "Shared": { "type": "number" }
            }
        });
        let result = adapter.compile_schema(&schema).unwrap();
        assert_eq!(result.value["properties"]["foo"]["type"], "string");
        assert_eq!(result.value["properties"]["bar"]["type"], "number");
    }

    #[test]
    fn test_compile_schema_external_ref_fails() {
        let adapter = GeminiSchemaAdapter::new();
        let schema = json!({
            "$ref": "https://example.com/schema.json"
        });
        let result = adapter.compile_schema(&schema);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Unresolved or external $ref"));
    }

    #[test]
    fn test_compile_schema_all_destructive_transforms_exact() {
        let adapter = GeminiSchemaAdapter::new();
        let keywords = [
            ("minimum", json!(0)),
            ("maximum", json!(10)),
            ("minLength", json!(1)),
            ("maxLength", json!(5)),
            ("pattern", json!(".*")),
            ("minItems", json!(1)),
            ("maxItems", json!(2)),
            ("uniqueItems", json!(true)),
            ("not", json!({"type": "string"})),
            ("if", json!({})),
        ];

        for (kw, val) in keywords {
            let schema = json!({ "type": "string", kw: val });
            let result = adapter.compile_schema(&schema);
            assert!(result.is_err(), "Should fail for keyword '{}' in Exact policy", kw);
        }
    }

    #[test]
    fn test_compile_schema_all_destructive_transforms_rv() {
        let adapter = GeminiSchemaAdapter::with_policy(ProjectionPolicy::RuntimeValidated);
        let keywords = [
            ("minimum", json!(0)),
            ("maximum", json!(10)),
            ("minLength", json!(1)),
            ("maxLength", json!(5)),
            ("pattern", json!(".*")),
            ("minItems", json!(1)),
            ("maxItems", json!(2)),
            ("uniqueItems", json!(true)),
        ];

        for (kw, val) in keywords {
            let schema = json!({ "type": "string", kw: val });
            let result = adapter.compile_schema(&schema).unwrap();
            assert!(result.diagnostics.iter().any(|d| d.keyword == kw));
            assert!(result.value.get(kw).is_none());
        }
    }

    #[test]
    fn test_compile_schema_cache_isolation() {
        let cache = adk_core::SchemaCache::new();
        let schema = json!({
            "type": "object",
            "properties": { "x": { "type": "string" } }
        });

        let studio = GeminiSchemaAdapter::new();
        let vertex = GeminiSchemaAdapter::vertex_ai();

        let res_studio = cache.get_or_compile(&schema, &studio).unwrap();
        let res_vertex = cache.get_or_compile(&schema, &vertex).unwrap();

        assert!(res_studio.value.get("additionalProperties").is_none());
        assert_eq!(res_vertex.value["additionalProperties"], false);
        assert_ne!(res_studio, res_vertex);
        assert_eq!(cache.len(), 2);
    }
}
