use crate::document::SchemaDocument;
use crate::role::SchemaRole;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// What a schema still requires, given a partial instance.
///
/// Validation answers whether a *complete* instance satisfies a schema. An agent
/// filling a tool call over several turns needs the other question: given what
/// is collected so far, what is still outstanding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Outstanding {
    /// Required keys absent from the instance.
    pub missing: BTreeSet<String>,
    /// Keys whose value would decide a conditional requirement that cannot yet
    /// be evaluated.
    ///
    /// A conditional whose condition is unknown is not waived. Naming the key
    /// that decides it is what lets a caller ask the question that resolves it.
    pub undecided: BTreeSet<String>,
}

impl Outstanding {
    /// Whether the instance satisfies every requirement the schema can decide.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.undecided.is_empty()
    }
}

/// How a conditional's `if` clause stands against a partial instance.
enum Condition {
    Matched,
    Unmatched,
    Undecided {
        /// Keys not yet supplied. Collecting them settles the condition, so
        /// they stop being outstanding once present.
        absent: BTreeSet<String>,
        /// Keys that *are* present but sit under a predicate this code cannot
        /// evaluate. Collecting them settles nothing, so they must survive the
        /// "already collected" filter — otherwise the obligation behind an
        /// unsupported condition silently disappears the moment its deciding
        /// field arrives.
        unsupported: BTreeSet<String>,
    },
}

impl<R: SchemaRole> SchemaDocument<R> {
    /// Reports what this schema still requires, given the fields collected so
    /// far.
    ///
    /// Reads `required`, `dependentRequired`, and `allOf` entries of the
    /// `if`/`then` form — all standard Draft 2020-12. Needs no compiled
    /// validator, so it is available without the `runtime-validation` feature.
    ///
    /// # Examples
    ///
    /// ```
    /// # use adk_schema::{IngestionPolicy, InputSchema, SchemaError};
    /// let schema = InputSchema::from_value(
    ///     serde_json::json!({
    ///         "properties": { "name": {}, "phone": {} },
    ///         "required": ["name", "phone"]
    ///     }),
    ///     &IngestionPolicy::default(),
    /// )?;
    ///
    /// let collected = serde_json::json!({ "name": "Ada" });
    /// let outstanding = schema.outstanding(collected.as_object().unwrap());
    ///
    /// assert!(outstanding.missing.contains("phone"));
    /// assert!(!outstanding.is_complete());
    /// # Ok::<(), SchemaError>(())
    /// ```
    pub fn outstanding(&self, collected: &Map<String, Value>) -> Outstanding {
        let mut result = Outstanding::default();
        let schema = self.as_value();

        for key in required_keys(schema) {
            if !collected.contains_key(key) {
                result.missing.insert(key.to_string());
            }
        }

        if let Some(dependent) = schema.get("dependentRequired").and_then(Value::as_object) {
            for (trigger, required) in dependent {
                if !collected.contains_key(trigger) {
                    continue;
                }
                for key in required.as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if !collected.contains_key(key) {
                        result.missing.insert(key.to_string());
                    }
                }
            }
        }

        let mut absent_gates = BTreeSet::new();
        let mut unsupported_gates = BTreeSet::new();

        for rule in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
            // A branch with no `if` is an unconditional intersection: its own
            // `required` is owed outright, not gated on anything.
            if rule.get("if").is_none() {
                for key in required_keys(rule) {
                    if !collected.contains_key(key) {
                        result.missing.insert(key.to_string());
                    }
                }
                continue;
            }

            match evaluate_condition(rule.get("if"), collected) {
                Condition::Matched => {
                    let then_keys = rule.get("then").map(required_keys).unwrap_or_default();
                    for key in then_keys {
                        if !collected.contains_key(key) {
                            result.missing.insert(key.to_string());
                        }
                    }
                }
                // A decided-false condition still carries the `else` obligation.
                Condition::Unmatched => {
                    let else_keys = rule.get("else").map(required_keys).unwrap_or_default();
                    for key in else_keys {
                        if !collected.contains_key(key) {
                            result.missing.insert(key.to_string());
                        }
                    }
                }
                Condition::Undecided { absent, unsupported } => {
                    absent_gates.extend(absent);
                    unsupported_gates.extend(unsupported);
                }
            }
        }

        // A gate that has since been collected is decided, however many other
        // rules still name it. Unsupported predicates are exempt: their key
        // being present does not make them evaluable.
        absent_gates.retain(|key| !collected.contains_key(key));
        result.undecided = absent_gates;
        result.undecided.extend(unsupported_gates);
        result
    }
}

fn required_keys(schema: &Value) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Evaluates an `if` clause against a partial instance.
///
/// Understands the `required`, `const`, and `enum` shapes that discriminate an
/// outcome. Anything else is reported as undecided rather than unmatched: an
/// unrecognised condition must not silently discharge the obligations behind it.
fn evaluate_condition(clause: Option<&Value>, collected: &Map<String, Value>) -> Condition {
    let Some(clause) = clause else {
        return Condition::Unmatched;
    };

    let mut absent = BTreeSet::new();
    let mut unsupported = BTreeSet::new();

    for key in required_keys(clause) {
        if !collected.contains_key(key) {
            absent.insert(key.to_string());
        }
    }

    if let Some(properties) = clause.get("properties").and_then(Value::as_object) {
        for (key, constraint) in properties {
            let Some(actual) = collected.get(key) else {
                absent.insert(key.clone());
                continue;
            };
            if let Some(expected) = constraint.get("const") {
                if actual != expected {
                    return Condition::Unmatched;
                }
            } else if let Some(options) = constraint.get("enum").and_then(Value::as_array) {
                if !options.contains(actual) {
                    return Condition::Unmatched;
                }
            } else {
                // The value is in hand but the predicate is one this code does
                // not model. Undecided, and no amount of collecting fixes it.
                unsupported.insert(key.clone());
            }
        }
    }

    if absent.is_empty() && unsupported.is_empty() {
        Condition::Matched
    } else {
        Condition::Undecided { absent, unsupported }
    }
}

#[cfg(test)]
mod tests {
    use crate::{IngestionPolicy, InputSchema};
    use serde_json::{Value, json};

    fn schema(value: Value) -> InputSchema {
        InputSchema::from_value(value, &IngestionPolicy::default()).expect("fixture ingests")
    }

    fn collected(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().expect("fixture is an object").clone()
    }

    mod outstanding {
        use super::*;

        #[test]
        fn reports_a_required_key_that_is_absent() {
            let result = schema(json!({ "required": ["name", "phone"] }))
                .outstanding(&collected(json!({ "name": "Ada" })));

            assert_eq!(result.missing, ["phone".to_string()].into());
        }

        #[test]
        fn reports_nothing_once_every_required_key_is_present() {
            let result = schema(json!({ "required": ["name"] }))
                .outstanding(&collected(json!({ "name": "Ada" })));

            assert!(result.is_complete(), "{result:?}");
        }

        #[test]
        fn applies_dependent_requirements_only_once_the_trigger_is_present() {
            let doc = schema(json!({ "dependentRequired": { "card": ["billing_address"] } }));

            let untriggered = doc.outstanding(&collected(json!({})));

            assert!(untriggered.is_complete(), "{untriggered:?}");
        }

        #[test]
        fn reports_a_dependent_requirement_once_its_trigger_arrives() {
            let result = schema(json!({ "dependentRequired": { "card": ["billing_address"] } }))
                .outstanding(&collected(json!({ "card": "4242" })));

            assert_eq!(result.missing, ["billing_address".to_string()].into());
        }
    }

    mod unconditional_all_of {
        use super::*;

        /// A branch with no `if` is a plain intersection, so its `required` is
        /// owed outright rather than gated on anything.
        #[test]
        fn unconditional_all_of_required_is_outstanding() {
            let result = schema(json!({ "allOf": [{ "required": ["caller_name"] }] }))
                .outstanding(&collected(json!({})));

            assert_eq!(result.missing, ["caller_name".to_string()].into());
        }

        #[test]
        fn unconditional_all_of_required_clears_once_collected() {
            let result = schema(json!({ "allOf": [{ "required": ["caller_name"] }] }))
                .outstanding(&collected(json!({ "caller_name": "Ada" })));

            assert!(result.is_complete(), "{result:?}");
        }
    }

    /// The `allOf`/`if`/`then` shape a conditional obligation projects to.
    mod conditional {
        use super::*;

        fn order_contract() -> InputSchema {
            schema(json!({
                "required": ["issue_description"],
                "allOf": [{
                    "if": {
                        "properties": { "request_kind": { "const": "order" } },
                        "required": ["request_kind"]
                    },
                    "then": { "required": ["caller_name", "fulfillment_method"] }
                }]
            }))
        }

        /// A condition whose deciding key is absent leaves its obligations
        /// pending, and names the key that would settle them.
        #[test]
        fn an_undecided_condition_reports_its_gate() {
            let result = order_contract()
                .outstanding(&collected(json!({ "issue_description": "asked about the lasagna" })));

            assert_eq!(result.undecided, ["request_kind".to_string()].into());
        }

        /// An undecided condition must not be treated as satisfied.
        #[test]
        fn an_undecided_condition_is_not_complete() {
            let result = order_contract()
                .outstanding(&collected(json!({ "issue_description": "asked about the lasagna" })));

            assert!(!result.is_complete(), "{result:?}");
        }

        #[test]
        fn a_matched_condition_reports_the_obligations_it_triggers() {
            let result = order_contract().outstanding(&collected(json!({
                "issue_description": "one lasagna",
                "request_kind": "order"
            })));

            assert_eq!(
                result.missing,
                ["caller_name".to_string(), "fulfillment_method".to_string()].into(),
            );
        }

        #[test]
        fn an_unmatched_condition_discharges_its_obligations() {
            let result = order_contract().outstanding(&collected(json!({
                "issue_description": "what are your hours",
                "request_kind": "information"
            })));

            assert!(result.is_complete(), "{result:?}");
        }

        /// A condition decided *false* still carries its `else` obligation.
        #[test]
        fn unmatched_condition_collects_else_required() {
            let result = schema(json!({
                "allOf": [{
                    "if": {
                        "properties": { "request_kind": { "const": "order" } },
                        "required": ["request_kind"]
                    },
                    "then": { "required": ["fulfillment_method"] },
                    "else": { "required": ["callback_number"] }
                }]
            }))
            .outstanding(&collected(json!({ "request_kind": "information" })));

            assert_eq!(result.missing, ["callback_number".to_string()].into());
        }

        /// The fail-open this guards: the deciding field is *present* but its
        /// predicate is unsupported, so the condition is still undecided. An
        /// "already collected" filter must not treat presence as resolution and
        /// discard the obligation behind it.
        #[test]
        fn unsupported_condition_with_present_gate_remains_undecided() {
            let result = schema(json!({
                "allOf": [{
                    "if": { "properties": { "total": { "minimum": 100 } } },
                    "then": { "required": ["approval"] }
                }]
            }))
            .outstanding(&collected(json!({ "total": 150 })));

            assert!(
                !result.is_complete(),
                "an unsupported condition was discharged by its gate being present: {result:?}",
            );
            assert_eq!(result.undecided, ["total".to_string()].into());
        }

        /// An `if` shape this code does not model must not silently waive the
        /// obligations behind it.
        #[test]
        fn an_unrecognised_condition_is_undecided_rather_than_waived() {
            let result = schema(json!({
                "allOf": [{
                    "if": { "properties": { "total": { "minimum": 100 } } },
                    "then": { "required": ["approval"] }
                }]
            }))
            .outstanding(&collected(json!({})));

            assert_eq!(result.undecided, ["total".to_string()].into());
        }
    }
}
