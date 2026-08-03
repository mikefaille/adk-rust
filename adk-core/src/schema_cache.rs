//! Schema normalization cache for LLM provider adapters.
use crate::SchemaAdapter;
use crate::schema_adapter::SchemaCompileError;
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

/// A thread-safe cache for normalized and compiled JSON Schemas.
#[derive(Debug, Default)]
pub struct SchemaCache {
    entries: Mutex<HashMap<u64, Value>>,
}

/// Adds a value to the hash in a fixed order.
///
/// - **Object keys are sorted first.** The order they were written in cannot
///   change the hash.
/// - **No JSON values are cloned.** Object members are collected into a
///   temporary `Vec` for sorting; this runs on every lookup, including hits.
/// - **Each kind of value gets its own marker.** The text `"1"` and the number
///   `1` hash differently.
fn hash_canonical(value: &Value, hasher: &mut DefaultHasher) {
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(flag) => {
            1u8.hash(hasher);
            flag.hash(hasher);
        }
        Value::Number(number) => {
            2u8.hash(hasher);
            // The textual form, because it is exact in every build. Going
            // through `as_f64` loses precision when `serde_json` is built with
            // `arbitrary_precision`, where a literal wider than f64 is kept
            // verbatim: distinct numbers would round together and share a cache
            // entry, and a literal outside f64 entirely would hash to nothing.
            number.to_string().hash(hasher);
        }
        Value::String(text) => {
            3u8.hash(hasher);
            text.hash(hasher);
        }
        Value::Array(items) => {
            4u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_canonical(item, hasher);
            }
        }
        Value::Object(members) => {
            5u8.hash(hasher);
            members.len().hash(hasher);
            let mut entries: Vec<(&String, &Value)> = members.iter().collect();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (key, member) in entries {
                key.hash(hasher);
                hash_canonical(member, hasher);
            }
        }
    }
}

impl SchemaCache {
    /// Creates a new empty schema cache.
    pub fn new() -> Self {
        Self { entries: Mutex::new(HashMap::new()) }
    }

    /// Returns the normalized schema for the given input, using the cache if available.
    pub fn get_or_normalize(&self, schema: &Value, adapter: &dyn SchemaAdapter) -> Value {
        let hash = Self::hash_schema_with_adapter(schema, adapter, "normalize");
        let mut cache = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.entry(hash).or_insert_with(|| adapter.normalize_schema(schema.clone())).clone()
    }

    /// Returns the compiled schema for the given input, using the cache if available.
    pub fn get_or_compile(
        &self,
        schema: &Value,
        adapter: &dyn SchemaAdapter,
    ) -> Result<Value, SchemaCompileError> {
        let hash = Self::hash_schema_with_adapter(schema, adapter, "compile");
        let mut cache = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(cached) = cache.get(&hash) {
            return Ok(cached.clone());
        }

        let compiled = adapter.compile_schema(schema)?;
        cache.insert(hash, compiled.clone());
        Ok(compiled)
    }

    /// Clears all cached entries.
    pub fn clear(&self) {
        let mut cache = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.clear();
    }

    /// Returns the number of cached entries.
    pub fn len(&self) -> usize {
        let cache = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.len()
    }

    /// Returns true if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn hash_schema_with_adapter(
        schema: &Value,
        adapter: &dyn SchemaAdapter,
        operation: &str,
    ) -> u64 {
        // Use a robust, collision-resistant identity for the cache key.
        let mut hasher = DefaultHasher::new();

        // 1. Identity of the schema content itself.
        // We use the JSON string representation as a stable, canonical identity.
        // Canonical, so the order the keys were written in cannot split one
        // schema across two entries. Replaces a `serde_json::to_vec` whose byte
        // order tracked insertion order under `preserve_order`, and whose
        // failure path collapsed every unserializable schema onto one key.
        hash_canonical(schema, &mut hasher);

        // 2. Identity of the compiler/adapter.
        adapter.identifier().hash(&mut hasher);
        adapter.version().hash(&mut hasher);
        adapter.surface().hash(&mut hasher);

        // 3. Identity of the operation type.
        operation.hash(&mut hasher);

        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GenericSchemaAdapter;
    use serde_json::json;

    #[derive(Debug)]
    struct MockAdapter(&'static str);
    impl crate::SchemaAdapter for MockAdapter {
        fn identifier(&self) -> &str {
            self.0
        }
        fn normalize_schema(&self, schema: Value) -> Value {
            schema
        }
    }

    #[test]
    fn test_cache_separation_by_adapter() {
        let cache = SchemaCache::new();
        let schema = json!({"type": "string"});

        let adapter1 = MockAdapter("a1");
        let adapter2 = MockAdapter("a2");

        // Use get_or_normalize to insert into cache
        cache.get_or_normalize(&schema, &adapter1);
        assert_eq!(cache.len(), 1);

        cache.get_or_normalize(&schema, &adapter2);
        assert_eq!(cache.len(), 2, "Cache should have separate entries for different adapters");
    }

    #[test]
    fn test_cache_separation_by_operation() {
        let cache = SchemaCache::new();
        let schema = json!({"type": "string"});
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&schema, &adapter);
        assert_eq!(cache.len(), 1);

        cache.get_or_compile(&schema, &adapter).unwrap();
        assert_eq!(cache.len(), 2, "Cache should have separate entries for normalize vs compile");
    }

    #[test]
    fn test_cache_hit() {
        let cache = SchemaCache::new();
        let schema = json!({"type": "string"});
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&schema, &adapter);
        assert_eq!(cache.len(), 1);

        cache.get_or_normalize(&schema, &adapter);
        assert_eq!(cache.len(), 1, "Cache should hit for same schema and adapter");
    }

    /// Two schemas differing only in key order are one schema, so they share a
    /// cache entry rather than each paying for normalization.
    #[test]
    fn key_order_does_not_create_a_second_entry() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(
            &json!({ "type": "object", "properties": { "a": {}, "b": {} } }),
            &adapter,
        );
        cache.get_or_normalize(
            &json!({ "properties": { "b": {}, "a": {} }, "type": "object" }),
            &adapter,
        );

        assert_eq!(cache.len(), 1, "key order must not change a schema's identity");
    }

    #[test]
    fn genuinely_different_schemas_keep_separate_entries() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&json!({ "type": "string" }), &adapter);
        cache.get_or_normalize(&json!({ "type": "integer" }), &adapter);

        assert_eq!(cache.len(), 2);
    }

    /// A property name containing pointer or escape syntax must not collapse
    /// two schemas onto one key.
    #[test]
    fn unusual_property_names_stay_distinct() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&json!({ "properties": { "a/b": {} } }), &adapter);
        cache.get_or_normalize(&json!({ "properties": { "a~b": {} } }), &adapter);

        assert_eq!(cache.len(), 2);
    }

    /// Text and a number that render alike must not collide.
    #[test]
    fn a_string_and_a_number_hash_differently() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&json!({ "const": "1" }), &adapter);
        cache.get_or_normalize(&json!({ "const": 1 }), &adapter);

        assert_eq!(cache.len(), 2);
    }

    /// Numbers are identified by their written form, so `5` and `5.0` are
    /// different schemas.
    ///
    /// This also closes an `arbitrary_precision` hazard that cannot be
    /// exercised here: with that feature a literal wider than f64 is kept
    /// verbatim, and identifying numbers by `as_f64` would round distinct
    /// values onto one cache entry. Without the feature `serde_json` already
    /// collapses such literals during parsing, so reproducing it would mean
    /// enabling the feature for every crate in the build.
    #[test]
    fn numbers_are_identified_by_their_written_form() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&json!({ "const": 5 }), &adapter);
        cache.get_or_normalize(&json!({ "const": 5.0 }), &adapter);

        assert_eq!(cache.len(), 2);
    }
}
