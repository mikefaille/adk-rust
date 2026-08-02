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
/// # Identity
///
/// An entry is keyed by four things: the schema's content, the adapter's
/// `identifier`, `version`, and `surface`, and which operation produced it.
/// Changing an adapter's version therefore invalidates its entries without
/// clearing the cache.
///
/// Schema content is compared **independently of object key order**: two
/// schemas that differ only in the order their members were written share an
/// entry. `serde_json` preserves insertion order here, so schemas arriving by
/// different routes — a re-fetched tool declaration, a republished contract —
/// otherwise miss the cache and pay for normalization again.
///
/// # Deliberate limits
///
/// - **Numbers are keyed by their written form.** `5` and `5.0` are distinct
///   keys. The effect is an extra miss, never a wrong hit, so the cache stays
///   conservative rather than adopting a semantic equality it cannot verify.
/// - **Keys are a 64-bit non-cryptographic hash.** A collision returns another
///   schema's normalized result, which the cache assumes will not occur; the
///   key is not a security boundary and must not be used as one.
/// - **Hashing recurses with schema depth.** `adk-core` applies no ingestion
///   bounds, so a caller accepting schemas from an untrusted source should
///   bound them before caching.
#[derive(Debug, Default)]
pub struct SchemaCache {
    entries: Mutex<HashMap<u64, Value>>,
}

/// Feeds a value to the hasher in canonical order.
///
/// Object members are visited by sorted key, so two schemas that differ only in
/// key order produce the same hash. Nothing is cloned or serialized: this runs
/// on every cache lookup, including hits, which is the path the cache exists to
/// keep cheap.
///
/// Each variant is tagged so values of different types cannot collide — the
/// string `"1"` and the number `1` hash differently.
fn hash_canonical(value: &Value, hasher: &mut DefaultHasher) {
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(flag) => {
            1u8.hash(hasher);
            flag.hash(hasher);
        }
        Value::Number(number) => {
            2u8.hash(hasher);
            // `Number` is not `Hash`. Integers keep their exact value; a float
            // hashes by its bits, which is sound here because `Value` cannot
            // hold NaN.
            if let Some(unsigned) = number.as_u64() {
                0u8.hash(hasher);
                unsigned.hash(hasher);
            } else if let Some(signed) = number.as_i64() {
                1u8.hash(hasher);
                signed.hash(hasher);
            } else if let Some(float) = number.as_f64() {
                2u8.hash(hasher);
                float.to_bits().hash(hasher);
            }
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
    /// Keyed by schema content, adapter identity, and this operation — see
    /// [`SchemaCache`] for what content identity covers.
    pub fn get_or_normalize(&self, schema: &Value, adapter: &dyn SchemaAdapter) -> Value {
        let hash = Self::hash_schema_with_adapter(schema, adapter, "normalize");
        let mut cache = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.entry(hash).or_insert_with(|| adapter.normalize_schema(schema.clone())).clone()
    }

    /// Returns the compiled schema for the given input, using the cache if
    /// available.
    ///
    /// Keyed separately from normalization, so both may be cached for one
    /// schema.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaCompileError`] when the adapter rejects the schema.
    /// Failures are not cached, so a rejected schema is recompiled on the next
    /// call.
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
    #[test]
    fn test_cache_separates_values_of_different_types() {
        let cache = SchemaCache::new();
        let adapter = GenericSchemaAdapter;

        cache.get_or_normalize(&json!({ "const": "1" }), &adapter);
        cache.get_or_normalize(&json!({ "const": 1 }), &adapter);

        assert_eq!(cache.len(), 2, "A string and a number must hash differently");
    }
}
