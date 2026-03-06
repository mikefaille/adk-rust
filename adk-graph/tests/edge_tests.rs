use adk_graph::edge::*;
use adk_graph::state::State;
use serde_json::json;

#[test]
fn test_edge_target_from_str() {
    let start: EdgeTarget = START.into();
    assert_eq!(start, EdgeTarget::from(START));

    let end: EdgeTarget = END.into();
    assert_eq!(end, EdgeTarget::from(END));

    let node: EdgeTarget = "my_node".into();
    assert_eq!(node, EdgeTarget::Node("my_node".to_string()));
}

#[test]
fn test_by_field_router() {
    let router = Router::by_field("action");

    let mut state = State::new();
    state.insert("action".to_string(), json!("save"));
    assert_eq!(router(&state), "save");

    state.insert("action".to_string(), json!("delete"));
    assert_eq!(router(&state), "delete");

    state.insert("action".to_string(), json!("unknown"));
    assert_eq!(router(&state), "unknown");

    state.insert("action".to_string(), json!(123));
    assert_eq!(router(&state), END);
}

#[test]
fn test_by_bool_router() {
    let router = Router::by_bool("is_valid", "success", "fail");

    let mut state = State::new();
    state.insert("is_valid".to_string(), json!(true));
    assert_eq!(router(&state), "success");

    state.insert("is_valid".to_string(), json!(false));
    assert_eq!(router(&state), "fail");

    state.insert("is_valid".to_string(), json!("not a bool"));
    assert_eq!(router(&state), "fail");
}

#[test]
fn test_max_iterations_router() {
    let router = Router::max_iterations("iteration_count", 5, "loop_node", "stop");

    let mut state = State::new();
    
    // No iteration count in state
    assert_eq!(router(&state), "loop_node");

    state.insert("iteration_count".to_string(), json!(3));
    assert_eq!(router(&state), "loop_node");

    state.insert("iteration_count".to_string(), json!(5));
    assert_eq!(router(&state), "stop");
}

#[test]
fn test_on_error_router() {
    let router = Router::on_error("error", "handle_error", "next");

    let mut state = State::new();
    
    // No error in state
    assert_eq!(router(&state), "next");

    state.insert("error".to_string(), json!("Something went wrong"));
    assert_eq!(router(&state), "handle_error");
}

#[test]
fn test_custom_router() {
    let router = Router::custom(|state| {
        let score = state.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        if score > 80 { "high".to_string() } else { "low".to_string() }
    });

    let mut state = State::new();
    state.insert("score".to_string(), json!(90));
    assert_eq!(router(&state), "high");

    state.insert("score".to_string(), json!(50));
    assert_eq!(router(&state), "low");
}


#[test]
fn test_edge_target_equality() {
    // Correct: Prove the constant converts to the explicit End variant
    assert_eq!(EdgeTarget::from(END), EdgeTarget::End);

    // Correct: Prove identical nodes are equal
    assert_eq!(EdgeTarget::Node("a".to_string()), EdgeTarget::Node("a".to_string()));

    // Correct: Prove distinct nodes are not equal
    assert_ne!(EdgeTarget::Node("a".to_string()), EdgeTarget::Node("b".to_string()));

    // Correct: Prove a node coincidentally named "end" is NOT the framework's End state
    assert_ne!(EdgeTarget::Node("end".to_string()), EdgeTarget::End);
}
