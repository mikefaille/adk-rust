//! Validation for UI components
//!
//! Server-side validation to catch malformed UiResponse before sending to client.

use crate::schema::*;

/// Validation error for UI components
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for ValidationError {}

/// Trait for validatable UI components
pub trait Validate {
    fn validate(&self, path: &str) -> Vec<ValidationError>;
}

impl Validate for UiResponse {
    fn validate(&self, path: &str) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.components.is_empty() {
            errors.push(ValidationError {
                path: path.to_string(),
                message: "UiResponse must have at least one component".to_string(),
            });
        }

        for (i, component) in self.components.iter().enumerate() {
            errors.extend(component.validate(&format!("{}.components[{}]", path, i)));
        }

        errors
    }
}

impl Validate for Component {
    fn validate(&self, path: &str) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        match self {
            Component::Text(t) => {
                if t.content.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.content", path),
                        message: "Text content cannot be empty".to_string(),
                    });
                }
            }
            Component::Button(b) => {
                if b.label.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.label", path),
                        message: "Button label cannot be empty".to_string(),
                    });
                }
                if b.action_id.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.action_id", path),
                        message: "Button action_id cannot be empty".to_string(),
                    });
                }
            }
            Component::TextInput(t) => {
                if t.name.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.name", path),
                        message: "TextInput name cannot be empty".to_string(),
                    });
                }
                if let (Some(min), Some(max)) = (t.min_length, t.max_length) {
                    if min > max {
                        errors.push(ValidationError {
                            path: format!("{}.min_length", path),
                            message: "min_length cannot be greater than max_length".to_string(),
                        });
                    }
                }
            }
            Component::NumberInput(n) => {
                if n.name.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.name", path),
                        message: "NumberInput name cannot be empty".to_string(),
                    });
                }
                if n.min > n.max {
                    errors.push(ValidationError {
                        path: format!("{}.min", path),
                        message: "min cannot be greater than max".to_string(),
                    });
                }
            }
            Component::Select(s) => {
                if s.name.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.name", path),
                        message: "Select name cannot be empty".to_string(),
                    });
                }
                if s.options.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.options", path),
                        message: "Select must have at least one option".to_string(),
                    });
                }
            }
            Component::Table(t) => {
                if t.columns.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.columns", path),
                        message: "Table must have at least one column".to_string(),
                    });
                }
            }
            Component::Chart(c) => {
                if c.data.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.data", path),
                        message: "Chart must have data".to_string(),
                    });
                }
                if c.y_keys.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.y_keys", path),
                        message: "Chart must have at least one y_key".to_string(),
                    });
                }
            }
            Component::Card(c) => {
                for (i, child) in c.content.iter().enumerate() {
                    errors.extend(child.validate(&format!("{}.content[{}]", path, i)));
                }
                if let Some(footer) = &c.footer {
                    for (i, child) in footer.iter().enumerate() {
                        errors.extend(child.validate(&format!("{}.footer[{}]", path, i)));
                    }
                }
            }
            Component::Modal(m) => {
                for (i, child) in m.content.iter().enumerate() {
                    errors.extend(child.validate(&format!("{}.content[{}]", path, i)));
                }
            }
            Component::Stack(s) => {
                for (i, child) in s.children.iter().enumerate() {
                    errors.extend(child.validate(&format!("{}.children[{}]", path, i)));
                }
            }
            Component::Grid(g) => {
                for (i, child) in g.children.iter().enumerate() {
                    errors.extend(child.validate(&format!("{}.children[{}]", path, i)));
                }
            }
            Component::Tabs(t) => {
                if t.tabs.is_empty() {
                    errors.push(ValidationError {
                        path: format!("{}.tabs", path),
                        message: "Tabs must have at least one tab".to_string(),
                    });
                }
            }
            // Other components with minimal validation
            _ => {}
        }

        errors
    }
}

/// Validate a UiResponse and return Result
pub fn validate_ui_response(ui: &UiResponse) -> Result<(), Vec<ValidationError>> {
    let errors = ui.validate("UiResponse");
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_response_fails() {
        let ui = UiResponse::new(vec![]);
        let result = validate_ui_response(&ui);
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_text_passes() {
        let ui = UiResponse::new(vec![Component::Text(Text {
            id: None,
            content: "Hello".to_string(),
            variant: TextVariant::Body,
        })]);
        let result = validate_ui_response(&ui);
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_button_label_fails() {
        let ui = UiResponse::new(vec![Component::Button(Button {
            id: None,
            label: "".to_string(),
            action_id: "click".to_string(),
            variant: ButtonVariant::Primary,
            disabled: false,
            icon: None,
        })]);
        let result = validate_ui_response(&ui);
        assert!(result.is_err());
    }
}
