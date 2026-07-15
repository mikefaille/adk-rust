use serde_json::Value;

pub fn add_implicit_object_type(schema: &mut Value) {
    if let Value::Object(obj) = schema {
        if obj.contains_key("properties") && !obj.contains_key("type") {
            obj.insert("type".to_string(), Value::String("object".to_string()));
        }
    }
}
