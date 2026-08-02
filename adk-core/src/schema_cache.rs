//! Schema normalization cache for LLM provider adapters.
//!
//! Provider adapters normalize a JSON Schema into the dialect their model
//! accepts, and [`SchemaAdapter::compile_schema`] may build a validator on top
//! of it. Both are expensive enough to be worth caching per turn.
use crate::SchemaAdapter;
use crate::schema_adapter::SchemaCompileError;
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

/// A thread-safe cache for normalized and compiled JSON Schemas.
///
/// # How entries are identified
///
/// | Part of the key | Comes from |
/// |---|---|
/// | The schema | its contents, ignoring the order the keys were written in |
/// | The adapter | its `identifier`, `version`, and `surface` |
/// | The operation | normalizing and compiling are stored separately |
///
/// Change an adapter's `version` and its old entries stop being used. You do
/// not need to clear the cache.
///
/// Key order is ignored on purpose. When `serde_json/preserve_order` is enabled
/// it keeps object keys in the order they were written, so the same schema built
/// two different ways would otherwise count as two schemas.
///
/// # Limits
///
/// | Limit | What it means for you |
/// |---|---|
/// | Numbers are compared as written | `5` and `5.0` count as different schemas. This costs an extra lookup. It never returns the wrong schema. |
/// | The key is a 64-bit hash | Two schemas could in theory share a key, and one would get the other's result. Do not rely on the key for security. |
/// | Hashing follows the schema's nesting | A deeply nested schema can use a lot of stack. `adk-core` does not limit schema size, so check schemas from untrusted sources first. |
#[derive(Debug, Default)]
pub struct SchemaCache {
    entries: Mutex<HashMap<u64, Value>>,
}

/// Adds a value to the hash in a fixed order.
///
/// - **Object keys are sorted first.** The order they were written in cannot
///   change the hash.
/// - **No JSON values are cloned.** Object members are collected into a
///   temporary `Vec` so they can be sorted. This runs on every lookup,
///   including ones that hit.
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
            // `Number` is not `Hash`, so hash its written form. That form is
            // exact in every build. Going through `as_u64`/`as_i64`/`as_f64`
            // is not: with `serde_json/arbitrary_precision` the literal is
            // kept as text, the integer accessors return `None` outside their
            // range, and `as_f64` rounds — so two schemas differing past
            // f64's precision would hash alike and the second would receive
            // the first one's normalized schema.
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
            // Sorted pairs rather than sorted keys plus a lookup: a member that
            // failed to resolve would drop out of the hash silently, and two
            // schemas sharing a key return each other's cached result.
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

    /// Returns the normalized schema for the given input, using the cache if
    /// available.
    ///
    /// See [`SchemaCache`] for how entries are identified.
    pub fn get_or_normalize(&self, schema: &Value, adapter: &dyn SchemaAdapter) -> Value {
        let hash = Self::hash_schema_with_adapter(schema, adapter, "normalize");
        let mut cache = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.entry(hash).or_insert_with(|| adapter.normalize_schema(schema.clone())).clone()
    }

    /// Returns the compiled schema for the given input, using the cache if
    /// available.
    ///
    /// Stored separately from the normalized form, so one schema can have
    /// both.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaCompileError`] if the adapter rejects the schema.
    /// Failures are not stored, so a rejected schema is compiled again on the
    /// next call.
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
        // Build a compact cache identity.
        let mut hasher = DefaultHasher::new();

        // 1. Identity of the schema content itself, independent of key order.
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

    #[test]
    fn test_cache_canonical_keys() {
        let cache = SchemaCache::new();
        // Two schemas differing only in key order
        let schema1 = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });
        let schema2 = json!({
            "type": "object",
            "required": ["name", "age"],
            "properties": {
                "age": {"type": "integer"},
                "name": {"type": "string"}
            }
        });
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&schema1, &adapter);
        assert_eq!(cache.len(), 1);

        cache.get_or_normalize(&schema2, &adapter);
        assert_eq!(cache.len(), 1, "Cache should hit because the schemas differ only in key order");
    }

    #[test]
    fn test_cache_genuinely_different_schemas() {
        let cache = SchemaCache::new();
        let schema1 = json!({"type": "string"});
        let schema2 = json!({"type": "integer"});
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&schema1, &adapter);
        assert_eq!(cache.len(), 1);

        cache.get_or_normalize(&schema2, &adapter);
        assert_eq!(
            cache.len(),
            2,
            "Genuinely different schemas should produce distinct cache entries"
        );
    }

    /// A tenant may name a property something that looks like pointer or
    /// escape syntax; the hash must still separate distinct schemas.
    #[test]
    fn test_cache_separates_schemas_with_unusual_keys() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&json!({ "properties": { "a/b": { "type": "string" } } }), &adapter);
        cache.get_or_normalize(&json!({ "properties": { "a~b": { "type": "string" } } }), &adapter);

        assert_eq!(cache.len(), 2, "Distinct schemas must not share a cache key");
    }

    /// A string and a number that render alike must not collide.
    /// Numbers are identified by their written form, so `5` and `5.0` are
    /// different schemas.
    ///
    /// This also closes an `arbitrary_precision` hazard that cannot be
    /// reproduced here: with that feature a literal wider than f64 is kept
    /// verbatim, and identifying numbers via `as_f64` would round distinct
    /// values onto one cache entry. Without the feature `serde_json` already
    /// collapses such literals while parsing, so exercising it would mean
    /// enabling the feature for every crate in the build.
    #[test]
    fn test_cache_identifies_numbers_by_written_form() {
        let cache = SchemaCache::new();
        let adapter = MockAdapter("m");

        cache.get_or_normalize(&json!({ "const": 5 }), &adapter);
        cache.get_or_normalize(&json!({ "const": 5.0 }), &adapter);

        assert_eq!(cache.len(), 2, "5 and 5.0 must not share a cache entry");
    }

    #[test]
    fn test_cache_separates_values_of_different_types() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&json!({ "const": "1" }), &adapter);
        cache.get_or_normalize(&json!({ "const": 1 }), &adapter);

        assert_eq!(cache.len(), 2, "A string and a number must hash differently");
    }
}
