use adk_graph::edge::*;
use adk_session::State;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_edge_target_from_str() {
    let start: EdgeTarget = "start".into();
    assert_eq!(start, EdgeTarget::Start);

    let end: EdgeTarget = "end".into();
    assert_eq!(end, EdgeTarget::End);

    let node: EdgeTarget = "my_node".into();
    assert_eq!(node, EdgeTarget::Node("my_node".to_string()));
}

#[test]
fn test_by_field_router() {
    let router = by_field("action", [("save", "save_node"), ("delete", "delete_node")]);

    let mut state = adk_session::InMemorySession::default();
    state.insert("action".to_string(), json!("save"));
    assert_eq!(router(&state), "save_node");

    state.insert("action".to_string(), json!("delete"));
    assert_eq!(router(&state), "delete_node");

    state.insert("action".to_string(), json!("unknown"));
    assert_eq!(router(&state), "end");

    state.insert("action".to_string(), json!(123));
    assert_eq!(router(&state), "end");
}

#[test]
fn test_by_bool_router() {
    let router = by_bool("is_valid", "success", "fail");

    let mut state = adk_session::InMemorySession::default();
    state.insert("is_valid".to_string(), json!(true));
    assert_eq!(router(&state), "success");

    state.insert("is_valid".to_string(), json!(false));
    assert_eq!(router(&state), "fail");

    state.insert("is_valid".to_string(), json!("not a bool"));
    assert_eq!(router(&state), "fail");
}

#[test]
fn test_max_iterations_router() {
    let router = max_iterations(5, "loop_node", "stop");

    let mut state = adk_session::InMemorySession::default();
    
    // No iteration count in state
    assert_eq!(router(&state), "loop_node");

    state.insert("iteration_count".to_string(), json!(3));
    assert_eq!(router(&state), "loop_node");

    state.insert("iteration_count".to_string(), json!(5));
    assert_eq!(router(&state), "stop");
}

#[test]
fn test_on_error_router() {
    let router = on_error("handle_error", "next");

    let mut state = adk_session::InMemorySession::default();
    
    // No error in state
    assert_eq!(router(&state), "next");

    state.insert("error".to_string(), json!("Something went wrong"));
    assert_eq!(router(&state), "handle_error");
}

#[test]
fn test_custom_router() {
    let router = custom_router(|state| {
        let score = state.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        if score > 80 { "high" } else { "low" }
    });

    let mut state = adk_session::InMemorySession::default();
    state.insert("score".to_string(), json!(90));
    assert_eq!(router(&state), "high");

    state.insert("score".to_string(), json!(50));
    assert_eq!(router(&state), "low");
}

#[test]
fn test_edge_target_equality() {
    assert_eq!(EdgeTarget::Start, EdgeTarget::Start);
    assert_eq!(EdgeTarget::End, EdgeTarget::End);
    assert_eq!(EdgeTarget::Node("a".to_string()), EdgeTarget::Node("a".to_string()));
    assert_ne!(EdgeTarget::Node("a".to_string()), EdgeTarget::Node("b".to_string()));
    assert_ne!(EdgeTarget::Node("end".to_string()), EdgeTarget::End);
}
