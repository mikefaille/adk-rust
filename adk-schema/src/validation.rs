use crate::document::SchemaDocument;
use crate::error::{Result, SchemaError, ValidationIssue};
use crate::policy::ValidationOptions;
use crate::role::SchemaRole;
use std::sync::Arc;

/// A compiled, validated schema document.
#[cfg(feature = "runtime-validation")]
#[cfg_attr(docsrs, doc(cfg(feature = "runtime-validation")))]
pub struct ValidatedSchemaDocument<R: SchemaRole> {
    document: SchemaDocument<R>,
    validator: Arc<jsonschema::Validator>,
}

#[cfg(feature = "runtime-validation")]
impl<R: SchemaRole> Clone for ValidatedSchemaDocument<R> {
    fn clone(&self) -> Self {
        Self { document: self.document.clone(), validator: self.validator.clone() }
    }
}

#[cfg(feature = "runtime-validation")]
impl<R: SchemaRole> std::fmt::Debug for ValidatedSchemaDocument<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatedSchemaDocument").field("document", &self.document).finish()
    }
}

#[cfg(feature = "runtime-validation")]
impl<R: SchemaRole> PartialEq for ValidatedSchemaDocument<R> {
    fn eq(&self, other: &Self) -> bool {
        self.document == other.document
    }
}

#[cfg(feature = "runtime-validation")]
impl<R: SchemaRole> ValidatedSchemaDocument<R> {
    /// Borrow the underlying SchemaDocument.
    pub fn as_document(&self) -> &SchemaDocument<R> {
        &self.document
    }
    /// Access the underlying canonical JSON Value.
    pub fn document(&self) -> &serde_json::Value {
        self.document.as_value()
    }
    /// Access the digest.
    pub fn digest(&self) -> crate::digest::SchemaDigest {
        self.document.digest()
    }

    /// Validates an instance against this schema under
    /// [`ValidationOptions::default`]: at most 100 issues, with the offending
    /// values masked out of the messages.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError::InvalidInstance`] if the instance does not
    /// satisfy the schema. Each issue carries the JSON pointer of the failing
    /// location; `truncated` reports whether the issue limit was reached.
    ///
    /// # Examples
    ///
    /// ```
    /// # use adk_schema::{IngestionPolicy, InputSchema, SchemaError};
    /// let schema = InputSchema::from_value(
    ///     serde_json::json!({ "properties": { "age": { "type": "integer" } } }),
    ///     &IngestionPolicy::default(),
    /// )?
    /// .compile()?;
    ///
    /// let err = schema
    ///     .validate(&serde_json::json!({ "age": "Jean-Marc" }))
    ///     .unwrap_err();
    ///
    /// // The pointer says where, without quoting what the caller supplied.
    /// assert!(!format!("{err:?}").contains("Jean-Marc"));
    /// # Ok::<(), SchemaError>(())
    /// ```
    pub fn validate(&self, instance: &serde_json::Value) -> Result<()> {
        self.validate_with(instance, &ValidationOptions::default())
    }

    /// Validates an instance under explicit [`ValidationOptions`].
    ///
    /// Enable [`ValidationOptions::include_instance_values`] only where the
    /// instance is known not to carry user data.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError::InvalidInstance`] if the instance does not
    /// satisfy the schema, carrying at most `options.max_issues` issues.
    pub fn validate_with(
        &self,
        instance: &serde_json::Value,
        options: &ValidationOptions,
    ) -> Result<()> {
        let mut issues = Vec::with_capacity(options.max_issues.min(16));
        let mut truncated = false;
        // Tracked separately from `issues`, because `max_issues` caps *reporting*
        // and must never cap *enforcement*. At `max_issues: 0` the loop breaks
        // before pushing anything, and deciding on `issues.is_empty()` would
        // return `Ok(())` for an instance the validator rejected.
        let mut rejected = false;

        for err in self.validator.iter_errors(instance) {
            rejected = true;
            if issues.len() >= options.max_issues {
                // `iter_errors` is lazy, so this also stops the work behind the
                // remaining errors.
                truncated = true;
                break;
            }
            issues.push(ValidationIssue {
                pointer: err.instance_path().to_string(),
                // `masked()` keeps the keyword and constraint, both schema-side,
                // and replaces the instance value with a placeholder.
                message: if options.include_instance_values {
                    err.to_string()
                } else {
                    err.masked().to_string()
                },
            });
        }

        if rejected {
            return Err(SchemaError::InvalidInstance { issues, truncated });
        }
        Ok(())
    }
}

#[cfg(feature = "runtime-validation")]
#[cfg_attr(docsrs, doc(cfg(feature = "runtime-validation")))]
impl<R: SchemaRole> SchemaDocument<R> {
    /// Compile this schema into a `ValidatedSchemaDocument`.
    pub fn compile(self) -> Result<ValidatedSchemaDocument<R>> {
        use jsonschema::{Draft, Validator};
        // Build validator options explicitly denying external resolution authority.
        // Since resolve-http and resolve-file features are not enabled,
        // no resolver handles these schemes.
        let validator = Validator::options()
            .with_draft(Draft::Draft202012)
            .build(self.as_value())
            .map_err(|e| {
                let issues = vec![ValidationIssue {
                    pointer: e.instance_path().to_string(),
                    message: e.to_string(),
                }];
                SchemaError::InvalidSchema { issues }
            })?;
        Ok(ValidatedSchemaDocument { document: self, validator: Arc::new(validator) })
    }
}

#[cfg(all(test, feature = "runtime-validation"))]
mod tests {
    use super::*;
    use crate::IngestionPolicy;
    use crate::InputSchema;
    use serde_json::json;

    #[test]
    fn test_validator_sharing_on_clone() {
        let schema = json!({
            "type": "object",
            "properties": {
                "foo": { "type": "string" }
            }
        });
        let policy = IngestionPolicy::default();
        let doc = InputSchema::from_value(schema, &policy).unwrap();
        let validated = doc.compile().unwrap();
        let cloned = validated.clone();
        assert!(Arc::ptr_eq(&validated.validator, &cloned.validator));
    }

    #[test]
    fn test_validate_object_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "intent": { "type": "string" }
            },
            "required": ["intent"]
        });
        let policy = IngestionPolicy::default();
        let doc = InputSchema::from_value(schema, &policy).unwrap();
        let validated = doc.compile().unwrap();
        assert!(validated.validate(&json!({"intent": "emergency"})).is_ok());
    }

    /// Fixtures generated from Rust types, so a fixture cannot drift from the
    /// shape it describes. Requires `schemars` for generation and
    /// `runtime-validation` (from the parent module) to validate.
    #[cfg(feature = "schemars")]
    mod typed {
        use super::*;

        #[derive(schemars::JsonSchema)]
        // Only the derived schema is ever used; no `Caller` is constructed.
        #[expect(dead_code, reason = "the field exists to shape the generated schema")]
        struct Caller {
            caller_name: i64,
        }

        /// A schema whose one field is an integer, so any string fails it.
        fn caller_schema() -> crate::ValidatedSchemaDocument<crate::Input> {
            InputSchema::for_type::<Caller>()
                .and_then(InputSchema::compile)
                .expect("a derived schema generates and compiles")
        }

        const CALLER_NAME: &str = "Jean-Marc Tremblay";

        fn caller_name_failure() -> SchemaError {
            caller_schema()
                .validate(&json!({ "caller_name": CALLER_NAME }))
                .expect_err("a string cannot satisfy an integer field")
        }

        mod validate {
            use super::*;

            /// Instances carry caller-supplied data, so the default must not
            /// quote the failing value.
            #[test]
            fn masks_the_failing_value_by_default() {
                let err = caller_name_failure();

                assert!(
                    !format!("{err} {err:?}").contains(CALLER_NAME),
                    "the failing value leaked into a default-rendered error: {err}",
                );
            }

            /// Masking retains the pointer, which locates the failing field.
            #[test]
            fn still_reports_which_field_failed() {
                let err = caller_name_failure();
                let SchemaError::InvalidInstance { issues, .. } = &err else {
                    panic!("expected an instance failure, got {err:?}");
                };

                assert!(
                    issues.iter().any(|issue| issue.pointer.contains("caller_name")),
                    "no issue pointed at the failing field: {issues:?}",
                );
            }
        }

        mod validate_with {
            /// `max_issues` caps reporting, never enforcement. At zero the
            /// validator still rejects; it just has nothing to say about why.
            #[test]
            fn max_issues_zero_still_rejects_invalid_instance() {
                let schema = InputSchema::for_type::<Caller>()
                    .and_then(InputSchema::compile)
                    .expect("type compiles");
                let options = ValidationOptions { max_issues: 0, ..Default::default() };

                let result = schema.validate_with(&serde_json::json!({}), &options);

                assert!(result.is_err(), "a zero reporting cap accepted an invalid instance");
            }

            #[test]
            fn max_issues_zero_reports_truncation() {
                let schema = InputSchema::for_type::<Caller>()
                    .and_then(InputSchema::compile)
                    .expect("type compiles");
                let options = ValidationOptions { max_issues: 0, ..Default::default() };

                match schema.validate_with(&serde_json::json!({}), &options) {
                    Err(SchemaError::InvalidInstance { issues, truncated }) => {
                        assert!(issues.is_empty());
                        assert!(truncated, "silence must be marked as truncation, not cleanliness");
                    }
                    other => panic!("expected InvalidInstance, got {other:?}"),
                }
            }

            use super::*;

            /// Opting in restores the value for fixture debugging.
            #[test]
            fn includes_the_failing_value_when_opted_in() {
                let options =
                    ValidationOptions { include_instance_values: true, ..Default::default() };

                let err = caller_schema()
                    .validate_with(&json!({ "caller_name": CALLER_NAME }), &options)
                    .expect_err("a string cannot satisfy an integer field");

                assert!(
                    format!("{err:?}").contains(CALLER_NAME),
                    "explicit opt-in did not include the value: {err:?}",
                );
            }

            /// Fifty wrong-typed entries capped at three.
            ///
            /// `HashMap<String, String>` generates `additionalProperties: {type:
            /// string}`, so each numeric entry is one failure.
            fn capped_failure() -> SchemaError {
                let schema = InputSchema::for_type::<std::collections::HashMap<String, String>>()
                    .and_then(InputSchema::compile)
                    .expect("a derived schema generates and compiles");
                let instance = json!(
                    (0..50).map(|i| (format!("f{i}"), json!(i))).collect::<serde_json::Map<_, _>>()
                );
                let options = ValidationOptions { max_issues: 3, ..Default::default() };

                schema
                    .validate_with(&instance, &options)
                    .expect_err("numbers cannot satisfy a string-valued map")
            }

            /// Instance validation is bounded, as ingestion already is.
            #[test]
            fn stops_collecting_at_the_issue_limit() {
                let SchemaError::InvalidInstance { issues, .. } = capped_failure() else {
                    panic!("expected an instance failure");
                };

                assert_eq!(issues.len(), 3, "collection ran past the configured limit");
            }

            /// A capped run reports the cap, so a short list is not mistaken
            /// for a complete one.
            #[test]
            fn reports_that_it_truncated() {
                let SchemaError::InvalidInstance { truncated, .. } = capped_failure() else {
                    panic!("expected an instance failure");
                };

                assert!(truncated, "a capped run did not report being capped");
            }
        }
    }
}
