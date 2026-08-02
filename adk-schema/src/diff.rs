use crate::document::SchemaDocument;
use crate::pointer::{child as child_pointer, last_token};
use crate::role::SchemaRole;
use serde_json::Value;

/// What a difference does to the set of instances a schema accepts.
///
/// Deliberately not an edit operation. `remove /title` and `remove /maximum`
/// are the same edit and different facts: one is cosmetic, the other stops a
/// constraint being enforced. A projection loss report needs the second
/// distinction and nothing at all from the first.
///
/// Full containment — whether one schema's accepted set truly includes
/// another's — is undecidable for JSON Schema in general. These variants
/// classify the fragment that can be decided keyword-locally and mark the rest
/// [`ConstraintChanged`](Self::ConstraintChanged) rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DifferenceKind {
    /// A constraint on the left is absent on the right: the right accepts more.
    ///
    /// On a projection this is a rule the runtime still enforces and the
    /// provider was never given.
    ConstraintRelaxed,
    /// A constraint on the right is absent on the left: the right accepts less.
    ConstraintTightened,
    /// A constraint exists on both sides with different values. The net effect
    /// on the accepted set is not computed.
    ConstraintChanged,
    /// The declared `type` differs. Called out separately from a changed
    /// constraint because it mutates the accepted set's shape rather than its
    /// bounds, and no bound comparison is meaningful across it.
    TypeMismatch,
    /// A non-validating keyword differs — `title`, `description`, `examples`,
    /// `$comment`, and the rest. No effect on which instances are accepted.
    AnnotationChanged,
}

/// Keywords that carry no validation force.
const ANNOTATIONS: &[&str] = &[
    "$comment",
    "default",
    "deprecated",
    "description",
    "examples",
    "readOnly",
    "title",
    "writeOnly",
];

/// One structural difference between two schema documents.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Difference {
    /// JSON pointer to the differing location.
    pub pointer: String,
    /// The keyword or member name at that location.
    pub keyword: String,
    /// How the two documents differ there.
    pub kind: DifferenceKind,
}

impl Difference {
    /// Whether this difference changes which instances are accepted.
    ///
    /// Filtering on this is what separates a projection's real losses from its
    /// cosmetic ones.
    pub fn affects_validation(&self) -> bool {
        self.kind != DifferenceKind::AnnotationChanged
    }

    /// Whether this difference leaves the far side enforcing less than the near
    /// side, when the diff compares a canonical schema against a **projection**
    /// of it.
    ///
    /// Independent of role, and the inverse of compatibility: a projection that
    /// relaxes an *input* is the dangerous case, because the runtime keeps
    /// validating the canonical contract while the model was shown a looser one
    /// and cannot know the rule it violates.
    pub fn leaves_enforcement_gap(&self) -> bool {
        matches!(
            self.kind,
            DifferenceKind::ConstraintRelaxed
                | DifferenceKind::ConstraintChanged
                | DifferenceKind::TypeMismatch
        )
    }
}

impl<R: SchemaRole> SchemaDocument<R> {
    /// Reports how this document differs from `other`.
    ///
    /// Both sides are canonical, so the comparison is order-independent and a
    /// schema never differs from itself.
    ///
    /// A provider projection is the motivating use: diffing a canonical schema
    /// against the reduced form sent to a model reports every constraint the
    /// reduction dropped, as [`DifferenceKind::ConstraintRelaxed`]. Those are
    /// the rules the runtime still enforces and the model was never shown.
    ///
    /// # Examples
    ///
    /// ```
    /// # use adk_schema::{DifferenceKind, IngestionPolicy, InputSchema, SchemaError};
    /// let canonical = InputSchema::from_value(
    ///     serde_json::json!({ "properties": { "n": { "type": "integer", "minimum": 1 } } }),
    ///     &IngestionPolicy::default(),
    /// )?;
    /// // A provider that cannot express `minimum` drops it.
    /// let projected = InputSchema::from_value(
    ///     serde_json::json!({ "properties": { "n": { "type": "integer" } } }),
    ///     &IngestionPolicy::default(),
    /// )?;
    ///
    /// let dropped = canonical.diff(&projected);
    ///
    /// assert_eq!(dropped.len(), 1);
    /// assert_eq!(dropped[0].keyword, "minimum");
    /// assert_eq!(dropped[0].kind, DifferenceKind::ConstraintRelaxed);
    /// # Ok::<(), SchemaError>(())
    /// ```
    pub fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut differences = Vec::new();
        walk(self.as_value(), other.as_value(), "", true, &mut differences);
        differences
    }
}

/// Compares two canonical documents.
///
/// `keyword_position` distinguishes a schema keyword from a member name: under
/// `properties`, `$defs`, `patternProperties`, or `dependentSchemas` the tokens
/// are names a tenant chose, so a property called `title` is not an annotation.
fn walk(
    left: &Value,
    right: &Value,
    pointer: &str,
    keyword_position: bool,
    out: &mut Vec<Difference>,
) {
    match (left, right) {
        (Value::Object(left_map), Value::Object(right_map)) => {
            for (key, left_value) in left_map {
                let child = child_pointer(pointer, key);
                match right_map.get(key) {
                    Some(right_value) => {
                        walk(left_value, right_value, &child, names_schemas(key), out)
                    }
                    None => out.push(Difference {
                        kind: classify(key, keyword_position, DifferenceKind::ConstraintRelaxed),
                        pointer: child,
                        keyword: key.clone(),
                    }),
                }
            }
            for key in right_map.keys().filter(|key| !left_map.contains_key(*key)) {
                out.push(Difference {
                    kind: classify(key, keyword_position, DifferenceKind::ConstraintTightened),
                    pointer: child_pointer(pointer, key),
                    keyword: key.clone(),
                });
            }
        }
        (Value::Array(left_items), Value::Array(right_items)) => {
            for (index, left_item) in left_items.iter().enumerate() {
                let child = format!("{pointer}/{index}");
                match right_items.get(index) {
                    Some(right_item) => walk(left_item, right_item, &child, false, out),
                    None => out.push(Difference {
                        pointer: child,
                        keyword: index.to_string(),
                        kind: DifferenceKind::ConstraintRelaxed,
                    }),
                }
            }
            for index in left_items.len()..right_items.len() {
                out.push(Difference {
                    pointer: format!("{pointer}/{index}"),
                    keyword: index.to_string(),
                    kind: DifferenceKind::ConstraintTightened,
                });
            }
        }
        _ if left != right => {
            let keyword = last_token(pointer);
            let changed = if keyword_position && keyword == "type" {
                DifferenceKind::TypeMismatch
            } else {
                DifferenceKind::ConstraintChanged
            };
            out.push(Difference {
                kind: classify(&keyword, keyword_position, changed),
                pointer: pointer.to_string(),
                keyword,
            });
        }
        _ => {}
    }
}

/// Whether a keyword's value is a map of named subschemas rather than more
/// keywords.
fn names_schemas(keyword: &str) -> bool {
    !matches!(keyword, "properties" | "$defs" | "patternProperties" | "dependentSchemas")
}

/// Annotations carry no validation force, but only where the token is actually
/// a keyword.
fn classify(keyword: &str, keyword_position: bool, otherwise: DifferenceKind) -> DifferenceKind {
    if keyword_position && ANNOTATIONS.contains(&keyword) {
        DifferenceKind::AnnotationChanged
    } else {
        otherwise
    }
}

#[cfg(test)]
mod tests {
    use crate::{Difference, DifferenceKind, IngestionPolicy, InputSchema};
    use serde_json::{Value, json};

    fn schema(value: Value) -> InputSchema {
        InputSchema::from_value(value, &IngestionPolicy::default()).expect("fixture ingests")
    }

    mod diff {
        use super::*;

        #[test]
        fn a_document_does_not_differ_from_itself() {
            let doc = schema(json!({ "properties": { "n": { "type": "integer" } } }));

            assert!(doc.diff(&doc).is_empty());
        }

        /// Canonicalization sorts keys, so key order cannot register as a
        /// difference.
        #[test]
        fn key_order_is_not_a_difference() {
            let left = schema(json!({ "a": 1, "b": 2 }));
            let right = schema(json!({ "b": 2, "a": 1 }));

            assert!(left.diff(&right).is_empty());
        }

        /// The motivating case: what a provider projection dropped.
        #[test]
        fn reports_a_constraint_the_projection_dropped() {
            let canonical =
                schema(json!({ "properties": { "n": { "type": "integer", "minimum": 1 } } }));
            let projected = schema(json!({ "properties": { "n": { "type": "integer" } } }));

            let dropped = canonical.diff(&projected);

            assert_eq!(dropped.len(), 1, "{dropped:?}");
            assert_eq!(dropped[0].kind, DifferenceKind::ConstraintRelaxed);
        }

        #[test]
        fn locates_the_dropped_constraint() {
            let canonical =
                schema(json!({ "properties": { "n": { "type": "integer", "minimum": 1 } } }));
            let projected = schema(json!({ "properties": { "n": { "type": "integer" } } }));

            let dropped = canonical.diff(&projected);

            assert_eq!(dropped[0].pointer, "/properties/n/minimum");
        }

        #[test]
        fn reports_an_addition_when_the_right_side_gained_a_keyword() {
            let left = schema(json!({ "properties": { "n": {} } }));
            let right = schema(json!({ "properties": { "n": { "type": "integer" } } }));

            let added = left.diff(&right);

            assert_eq!(added[0].kind, DifferenceKind::ConstraintTightened);
        }

        #[test]
        fn reports_a_changed_value_in_place() {
            let left = schema(json!({ "properties": { "n": { "type": "integer" } } }));
            let right = schema(json!({ "properties": { "n": { "type": "string" } } }));

            let changed = left.diff(&right);

            assert_eq!(changed[0].kind, DifferenceKind::TypeMismatch);
        }

        /// A pointer must survive a member name containing pointer syntax.
        #[test]
        fn escapes_pointer_tokens_in_member_names() {
            let left = schema(json!({ "properties": { "a/b": { "type": "integer" } } }));
            let right = schema(json!({ "properties": { "a/b": {} } }));

            let dropped = left.diff(&right);

            assert_eq!(dropped[0].pointer, "/properties/a~1b/type");
        }
    }

    mod effect {
        use super::*;

        /// The distinction the reframing exists for: dropping `title` and
        /// dropping `maximum` are the same edit and different facts.
        #[test]
        fn a_dropped_annotation_does_not_affect_validation() {
            let left = schema(json!({ "properties": { "n": { "title": "Count" } } }));
            let right = schema(json!({ "properties": { "n": {} } }));

            let differences = left.diff(&right);

            assert!(!differences[0].affects_validation(), "{differences:?}");
        }

        #[test]
        fn a_dropped_constraint_affects_validation() {
            let left = schema(json!({ "properties": { "n": { "maximum": 10 } } }));
            let right = schema(json!({ "properties": { "n": {} } }));

            let differences = left.diff(&right);

            assert!(differences[0].affects_validation(), "{differences:?}");
        }

        /// A tenant may name a property `title`. Only keyword positions are
        /// annotations.
        #[test]
        fn a_property_named_like_an_annotation_is_not_one() {
            let left = schema(json!({ "properties": { "title": { "type": "string" } } }));
            let right = schema(json!({ "properties": {} }));

            let differences = left.diff(&right);

            assert!(
                differences[0].affects_validation(),
                "a property named `title` is a constraint, not an annotation: {differences:?}",
            );
        }
    }

    /// The distinction the review surfaced: compatibility and enforcement
    /// divergence are different questions, and a relaxed input answers them
    /// oppositely.
    mod enforcement_gap {
        use super::*;

        fn relaxed_input() -> Difference {
            Difference {
                pointer: "/properties/n/minimum".to_string(),
                keyword: "minimum".to_string(),
                kind: DifferenceKind::ConstraintRelaxed,
            }
        }

        #[test]
        fn a_relaxed_input_still_leaves_an_enforcement_gap() {
            assert!(
                relaxed_input().leaves_enforcement_gap(),
                "a projection that drops an input constraint under-tells the model",
            );
        }

        #[test]
        fn an_annotation_leaves_no_enforcement_gap() {
            let difference = Difference {
                pointer: "/properties/n/title".to_string(),
                keyword: "title".to_string(),
                kind: DifferenceKind::AnnotationChanged,
            };

            assert!(!difference.leaves_enforcement_gap());
        }
    }
}
