use crate::document::SchemaMetrics;
use crate::error::{LimitKind, ReferenceRejection, Result, SchemaError};
use crate::policy::IngestionPolicy;
use crate::references::{parse_local_ref, resolve_local_pointer};
use serde_json::Value;
use std::collections::HashMap;

pub(crate) struct StructuralScan {
    pub(crate) metrics: SchemaMetrics,
    pub(crate) references: Vec<ReferenceEdge>,
}

pub(crate) struct ReferenceEdge {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) raw: String,
}

struct ScanFrame<'a> {
    value: &'a Value,
    pointer: String,
    depth: usize,
}

pub(crate) fn scan_structure(root: &Value, policy: &IngestionPolicy) -> Result<StructuralScan> {
    let mut stack = vec![ScanFrame { value: root, pointer: String::new(), depth: 1 }];
    let mut node_count = 0;
    let mut reference_count = 0;
    let mut max_depth = 0;
    let mut references = Vec::new();

    while let Some(frame) = stack.pop() {
        node_count += 1;
        if node_count > policy.max_nodes {
            return Err(SchemaError::LimitExceeded {
                kind: LimitKind::NodeCount,
                limit: policy.max_nodes,
                observed: node_count,
                pointer: frame.pointer,
            });
        }
        if frame.depth > policy.max_depth {
            return Err(SchemaError::LimitExceeded {
                kind: LimitKind::NestingDepth,
                limit: policy.max_depth,
                observed: frame.depth,
                pointer: frame.pointer,
            });
        }
        if frame.depth > max_depth {
            max_depth = frame.depth;
        }

        match frame.value {
            Value::Object(map) => {
                for (key, val) in map {
                    let next_pointer = if frame.pointer.is_empty() {
                        format!("/{}", key.replace('~', "~0").replace('/', "~1"))
                    } else {
                        format!("{}/{}", frame.pointer, key.replace('~', "~0").replace('/', "~1"))
                    };

                    if key == "$anchor" || key == "$dynamicAnchor" {
                        return Err(SchemaError::UnsupportedReference {
                            pointer: next_pointer,
                            reference: key.clone(),
                            reason: ReferenceRejection::UnsupportedAnchor,
                        });
                    }
                    if key == "$dynamicRef" {
                        return Err(SchemaError::UnsupportedReference {
                            pointer: next_pointer,
                            reference: key.clone(),
                            reason: ReferenceRejection::UnsupportedDynamicRef,
                        });
                    }

                    if key == "$ref" {
                        reference_count += 1;
                        if reference_count > policy.max_references {
                            return Err(SchemaError::LimitExceeded {
                                kind: LimitKind::ReferenceCount,
                                limit: policy.max_references,
                                observed: reference_count,
                                pointer: next_pointer,
                            });
                        }
                        let ref_str = val.as_str().ok_or_else(|| SchemaError::Parse {
                            message: "$ref value must be a string".to_string(),
                        })?;
                        let target_path = parse_local_ref(ref_str).map_err(|reason| {
                            SchemaError::UnsupportedReference {
                                pointer: next_pointer.clone(),
                                reference: ref_str.to_string(),
                                reason,
                            }
                        })?;
                        references.push(ReferenceEdge {
                            source: frame.pointer.clone(),
                            target: target_path,
                            raw: ref_str.to_string(),
                        });
                    } else {
                        stack.push(ScanFrame {
                            value: val,
                            pointer: next_pointer,
                            depth: frame.depth + 1,
                        });
                    }
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    let next_pointer = format!("{}/{}", frame.pointer, i);
                    stack.push(ScanFrame {
                        value: val,
                        pointer: next_pointer,
                        depth: frame.depth + 1,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(StructuralScan {
        metrics: SchemaMetrics { depth: max_depth, node_count, reference_count },
        references,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Unvisited,
    Visiting,
    Visited,
}

pub(crate) fn validate_reference_graph(
    root: &Value,
    references: &[ReferenceEdge],
    _policy: &IngestionPolicy,
) -> Result<()> {
    // 1. Verify all targets exist
    for edge in references {
        resolve_local_pointer(root, &edge.target).map_err(|_| SchemaError::MissingReference {
            pointer: edge.source.clone(),
            reference: edge.raw.clone(),
        })?;
    }

    // 2. Cycle detection using three-state DFS
    let mut states: HashMap<String, VisitState> = HashMap::new();
    for edge in references {
        states.insert(edge.target.clone(), VisitState::Unvisited);
        states.insert(edge.source.clone(), VisitState::Unvisited);
    }
    states.insert(String::new(), VisitState::Unvisited);

    let keys: Vec<String> = states.keys().cloned().collect();
    for node in keys {
        if states.get(&node) == Some(&VisitState::Unvisited) {
            let mut stack = vec![(node.clone(), false)];
            while let Some((curr, is_backtrack)) = stack.pop() {
                if is_backtrack {
                    states.insert(curr, VisitState::Visited);
                } else {
                    match states.get(&curr) {
                        Some(&VisitState::Visiting) => {
                            let cycle: Vec<String> = stack
                                .iter()
                                .filter(|(_, backtrack)| !backtrack)
                                .map(|(p, _)| p.clone())
                                .collect();
                            return Err(SchemaError::ReferenceCycle { cycle });
                        }
                        Some(&VisitState::Visited) => {}
                        _ => {
                            states.insert(curr.clone(), VisitState::Visiting);
                            stack.push((curr.clone(), true));
                            for edge in references {
                                let is_child = if curr.is_empty() {
                                    true
                                } else {
                                    edge.source == curr
                                        || edge.source.starts_with(&format!("{}/", curr))
                                };
                                if is_child
                                    && states.get(&edge.target) != Some(&VisitState::Visited)
                                {
                                    stack.push((edge.target.clone(), false));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
