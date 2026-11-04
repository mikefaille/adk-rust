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

/// Whether a keyword's value constrains nothing, making it equivalent to the
/// keyword being absent.
///
/// `required: []` demands nothing and `properties: {}` restricts nothing, so
/// writing either is the same as omitting it. Reporting the omission as a
/// dropped constraint is representational noise of exactly the kind
/// [`Difference::affects_validation`] exists to filter out.
///
/// `enum: []` is deliberately excluded: an empty `enum` accepts *nothing*, so
/// it is the strongest constraint expressible, not a vacuous one.
fn is_vacuous(keyword: &str, value: &Value) -> bool {
    match value {
        Value::Array(items) => items.is_empty() && keyword != "enum",
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// Rewrites a single-element `type` array to the scalar it means.
///
/// `{"type": ["string"]}` and `{"type": "string"}` are the same schema, and
/// provider adapters routinely collapse one into the other — Gemini's does it
/// unconditionally. Comparing them literally reports a `TypeMismatch`, which
/// `leaves_enforcement_gap()` then treats as dangerous.
fn normalized_type(value: &Value) -> &Value {
    match value {
        Value::Array(items) if items.len() == 1 => &items[0],
        other => other,
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
                        // `["string"]` and `"string"` are one schema written two
                        // ways, so compare what the keyword means.
                        let (left_value, right_value) = if keyword_position && key == "type" {
                            (normalized_type(left_value), normalized_type(right_value))
                        } else {
                            (left_value, right_value)
                        };
                        walk(left_value, right_value, &child, names_schemas(key), out)
                    }
                    // A vacuous keyword is equivalent to its own absence.
                    None if keyword_position && is_vacuous(key, left_value) => {}
                    None => out.push(Difference {
                        kind: classify(key, keyword_position, DifferenceKind::ConstraintRelaxed),
                        pointer: child,
                        keyword: key.clone(),
                    }),
                }
            }
            for (key, right_value) in
                right_map.iter().filter(|(key, _)| !left_map.contains_key(*key))
            {
                if keyword_position && is_vacuous(key, right_value) {
                    continue;
                }
                out.push(Difference {
                    kind: classify(key, keyword_position, DifferenceKind::ConstraintTightened),
                    pointer: child_pointer(pointer, key),
                    keyword: key.clone(),
                });
            }
        }
        (Value::Array(left_items), Value::Array(right_items)) => {
            let keyword = last_token(pointer);

            // `required` and `enum` are sets written as arrays. Comparing them
            // positionally reports a reorder as a constraint change, which is
            // exactly the representational noise `affects_validation()` exists
            // to filter out.
            if UNORDERED.contains(&keyword.as_str()) {
                compare_unordered(
                    left_items,
                    right_items,
                    pointer,
                    &keyword,
                    keyword_position,
                    out,
                );
                return;
            }

            // Members of `allOf`/`anyOf`/`oneOf`/`prefixItems` are schemas, so
            // their keys are keywords — without this an annotation nested in a
            // branch is misreported as a dropped constraint.
            let members_are_schemas = SCHEMA_ARRAYS.contains(&keyword.as_str());

            for (index, left_item) in left_items.iter().enumerate() {
                let child = format!("{pointer}/{index}");
                match right_items.get(index) {
                    Some(right_item) => {
                        walk(left_item, right_item, &child, members_are_schemas, out)
                    }
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
/// Array-valued keywords whose members are schemas, not plain values.
const SCHEMA_ARRAYS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// Array-valued keywords that denote sets, where order carries no meaning.
const UNORDERED: &[&str] = &["required", "enum", "type"];

/// Compares two set-valued keywords by membership.
///
/// A value only on the left is a constraint the right no longer states; a value
/// only on the right is one it added. Order alone produces nothing.
fn compare_unordered(
    left_items: &[Value],
    right_items: &[Value],
    pointer: &str,
    keyword: &str,
    keyword_position: bool,
    out: &mut Vec<Difference>,
) {
    // `enum` and a `type` union both *list what is accepted*, so dropping a
    // member accepts less — tightened. `required` lists what is demanded, so
    // dropping one demands less — relaxed. The two run opposite ways.
    let dropped_from_left = if matches!(keyword, "enum" | "type") {
        DifferenceKind::ConstraintTightened
    } else {
        DifferenceKind::ConstraintRelaxed
    };
    let added_on_right = if matches!(keyword, "enum" | "type") {
        DifferenceKind::ConstraintRelaxed
    } else {
        DifferenceKind::ConstraintTightened
    };

    let mut report = |kind| {
        out.push(Difference {
            kind: classify(keyword, keyword_position, kind),
            pointer: pointer.to_string(),
            keyword: keyword.to_string(),
        });
    };

    if left_items.iter().any(|value| !right_items.contains(value)) {
        report(dropped_from_left);
    }
    if right_items.iter().any(|value| !left_items.contains(value)) {
        report(added_on_right);
    }
}

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

    mod representational_noise {
        use super::*;

        /// `["string"]` and `"string"` are one schema written two ways, and
        /// provider adapters collapse one into the other routinely.
        #[test]
        fn a_single_element_type_array_equals_the_scalar() {
            let losses =
                schema(json!({ "type": ["string"] })).diff(&schema(json!({ "type": "string" })));

            assert!(
                losses.is_empty(),
                "a spelling of `type` was reported as a change: {losses:#?}"
            );
        }

        /// A genuinely narrowed union must still be caught.
        #[test]
        fn a_narrowed_type_union_is_still_reported() {
            let losses = schema(json!({ "type": ["string", "null"] }))
                .diff(&schema(json!({ "type": "string" })));

            assert!(!losses.is_empty(), "dropping `null` from a union went unreported");
        }

        /// `required: []` demands nothing, so omitting it drops no constraint.
        #[test]
        fn an_empty_required_equals_its_absence() {
            let losses = schema(json!({ "required": [] })).diff(&schema(json!({})));

            assert!(losses.is_empty(), "{losses:#?}");
        }

        /// `properties: {}` restricts nothing.
        #[test]
        fn an_empty_properties_equals_its_absence() {
            let losses = schema(json!({ "properties": {} })).diff(&schema(json!({})));

            assert!(losses.is_empty(), "{losses:#?}");
        }

        /// `enum: []` is the opposite case: it accepts *nothing*, so it is the
        /// strongest constraint expressible and dropping it is a real relaxation.
        #[test]
        fn an_empty_enum_is_not_vacuous() {
            let losses = schema(json!({ "enum": [] })).diff(&schema(json!({})));

            assert!(
                losses.iter().any(Difference::leaves_enforcement_gap),
                "dropping an unsatisfiable `enum` was treated as noise: {losses:#?}",
            );
        }

        /// Members of `allOf` are schemas, so their keys are keywords. Without
        /// that context a nested `description` reads as a dropped constraint.
        #[test]
        fn annotation_inside_all_of_is_not_an_enforcement_gap() {
            let left = schema(json!({ "allOf": [{ "description": "before", "minimum": 1 }] }));
            let right = schema(json!({ "allOf": [{ "description": "after", "minimum": 1 }] }));

            let losses = left.diff(&right);

            assert!(
                !losses.iter().any(Difference::leaves_enforcement_gap),
                "an annotation change was reported as an enforcement gap: {losses:#?}",
            );
        }

        /// `required` and `enum` are sets written as arrays; reordering them
        /// accepts exactly the same instances.
        #[test]
        fn reordered_required_and_enum_arrays_do_not_create_false_losses() {
            let left = schema(json!({
                "required": ["a", "b"],
                "properties": { "k": { "enum": ["x", "y"] } }
            }));
            let right = schema(json!({
                "required": ["b", "a"],
                "properties": { "k": { "enum": ["y", "x"] } }
            }));

            let losses = left.diff(&right);

            assert!(losses.is_empty(), "a reorder was reported as a difference: {losses:#?}");
        }

        /// The set comparison must still catch a genuine removal.
        #[test]
        fn a_dropped_required_key_is_still_reported() {
            let left = schema(json!({ "required": ["a", "b"] }));
            let right = schema(json!({ "required": ["a"] }));

            let losses = left.diff(&right);

            assert!(
                losses.iter().any(|loss| loss.keyword == "required"
                    && loss.kind == DifferenceKind::ConstraintRelaxed),
                "dropping a required key was not reported as relaxed: {losses:#?}",
            );
        }

        /// And a narrowed `enum` runs the other way: fewer accepted values is a
        /// tightening, not a relaxation.
        #[test]
        fn a_narrowed_enum_is_reported_as_tightened() {
            let left = schema(json!({ "enum": ["x", "y"] }));
            let right = schema(json!({ "enum": ["x"] }));

            let losses = left.diff(&right);

            assert!(
                losses.iter().any(|loss| loss.kind == DifferenceKind::ConstraintTightened),
                "a narrowed enum was not reported as tightened: {losses:#?}",
            );
        }
    }
}
