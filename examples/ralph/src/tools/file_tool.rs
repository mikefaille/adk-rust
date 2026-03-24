//! File operations tool

use adk_core::{AdkError, Result, Tool, ToolContext};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// Tool for file operations
pub struct FileTool {
    base_path: String,
}

impl FileTool {
    pub fn new(base_path: String) -> Self {
        Self { base_path }
    }

    fn resolve_safe_path(&self, path_str: &str) -> Result<std::path::PathBuf> {
        let path = Path::new(path_str);
        if path.is_absolute() {
            return Err(AdkError::Tool("Absolute paths are not allowed".to_string()));
        }

        let base = Path::new(&self.base_path).canonicalize().map_err(|e| {
            AdkError::Tool(format!("Failed to canonicalize base path: {}", e))
        })?;

        let mut full_path = base.clone();
        for component in path.components() {
            match component {
                std::path::Component::Normal(c) => full_path.push(c),
                std::path::Component::ParentDir => {
                    if !full_path.pop() || !full_path.starts_with(&base) {
                        return Err(AdkError::Tool("Path traversal attempt detected".to_string()));
                    }
                }
                std::path::Component::CurDir => {}
                _ => return Err(AdkError::Tool(format!("Invalid path component: {:?}", component))),
            }
        }

        if !full_path.starts_with(&base) {
            return Err(AdkError::Tool("Path traversal detected".to_string()));
        }

        Ok(full_path)
    }
}

#[async_trait]
impl Tool for FileTool {
    fn name(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        "File operations: read, write, append, list"
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["read", "write", "append", "list"],
                    "description": "File operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory path (relative to project root)"
                },
                "content": {
                    "type": "string",
                    "description": "Content for write/append operations"
                }
            },
            "required": ["operation", "path"]
        }))
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, params: Value) -> Result<Value> {
        let operation = params["operation"]
            .as_str()
            .ok_or_else(|| AdkError::Tool("Missing operation".to_string()))?;
        let path_str =
            params["path"].as_str().ok_or_else(|| AdkError::Tool("Missing path".to_string()))?;

        let full_path = self.resolve_safe_path(path_str)?;

        match operation {
            "read" => {
                let content = fs::read_to_string(&full_path)
                    .map_err(|e| AdkError::Tool(format!("Failed to read file: {}", e)))?;
                Ok(json!({
                    "path": path_str,
                    "content": content
                }))
            }
            "write" => {
                let content = params["content"]
                    .as_str()
                    .ok_or_else(|| AdkError::Tool("Missing content".to_string()))?;

                // Create parent directories if they don't exist
                if let Some(parent) = full_path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| AdkError::Tool(format!("Failed to create directory: {}", e)))?;
                }

                fs::write(&full_path, content)
                    .map_err(|e| AdkError::Tool(format!("Failed to write file: {}", e)))?;
                Ok(json!({
                    "status": "written",
                    "path": path_str
                }))
            }
            "append" => {
                let content = params["content"]
                    .as_str()
                    .ok_or_else(|| AdkError::Tool("Missing content".to_string()))?;

                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&full_path)
                    .map_err(|e| AdkError::Tool(format!("Failed to open file for append: {}", e)))?;

                file.write_all(content.as_bytes())
                    .map_err(|e| AdkError::Tool(format!("Failed to append to file: {}", e)))?;

                Ok(json!({
                    "status": "appended",
                    "path": path_str
                }))
            }
            "list" => {
                let entries: Vec<String> = fs::read_dir(&full_path)
                    .map_err(|e| AdkError::Tool(format!("Failed to read directory: {}", e)))?
                    .filter_map(|entry| {
                        entry.ok().and_then(|e| {
                            e.file_name().to_str().map(|s| {
                                if e.path().is_dir() { format!("{}/", s) } else { s.to_string() }
                            })
                        })
                    })
                    .collect();

                Ok(json!({
                    "path": path_str,
                    "entries": entries
                }))
            }
            _ => Err(AdkError::Tool(format!("Unknown operation: {}", operation))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::EventActions;

    struct MockContext;
    #[async_trait]
    impl adk_core::ReadonlyContext for MockContext {
        fn identity(&self) -> &adk_core::types::AdkIdentity {
            static IDENTITY: std::sync::OnceLock<adk_core::types::AdkIdentity> =
                std::sync::OnceLock::new();
            IDENTITY.get_or_init(adk_core::types::AdkIdentity::default)
        }
        fn user_content(&self) -> &adk_core::Content {
            static CONTENT: std::sync::OnceLock<adk_core::Content> = std::sync::OnceLock::new();
            CONTENT.get_or_init(|| adk_core::Content::new(adk_core::types::Role::User))
        }
        fn metadata(&self) -> &std::collections::HashMap<String, String> {
            static METADATA: std::sync::OnceLock<std::collections::HashMap<String, String>> =
                std::sync::OnceLock::new();
            METADATA.get_or_init(std::collections::HashMap::new)
        }
    }
    #[async_trait]
    impl adk_core::CallbackContext for MockContext {
        fn artifacts(&self) -> Option<Arc<dyn adk_core::Artifacts>> {
            None
        }
    }
    #[async_trait]
    impl ToolContext for MockContext {
        fn function_call_id(&self) -> &str {
            "test"
        }
        fn actions(&self) -> EventActions {
            EventActions::default()
        }
        fn set_actions(&self, _actions: EventActions) {}
        async fn search_memory(&self, _query: &str) -> Result<Vec<adk_core::MemoryEntry>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn test_path_traversal_read() {
        let base_dir = std::env::current_dir().unwrap().join("test_base_read");
        fs::create_dir_all(&base_dir).unwrap();

        let outside_file = base_dir.parent().unwrap().join("traversal_test_read.txt");
        fs::write(&outside_file, "secret content").unwrap();

        let tool = FileTool::new(base_dir.to_str().unwrap().to_string());
        let ctx = Arc::new(MockContext);

        let params = json!({
            "operation": "read",
            "path": "../traversal_test_read.txt"
        });

        let result = tool.execute(ctx, params).await;

        // Cleanup
        let _ = fs::remove_file(&outside_file);
        let _ = fs::remove_dir_all(&base_dir);

        assert!(result.is_err(), "Vulnerability still exists: path traversal succeeded");
        let err = result.err().unwrap().to_string();
        assert!(err.contains("Path traversal attempt detected"));
    }

    #[tokio::test]
    async fn test_safe_read() {
        let base_dir = std::env::current_dir().unwrap().join("test_base_safe");
        fs::create_dir_all(&base_dir).unwrap();

        let safe_file = base_dir.join("safe.txt");
        fs::write(&safe_file, "safe content").unwrap();

        let tool = FileTool::new(base_dir.to_str().unwrap().to_string());
        let ctx = Arc::new(MockContext);

        let params = json!({
            "operation": "read",
            "path": "safe.txt"
        });

        let result = tool.execute(ctx, params).await;

        // Cleanup
        let _ = fs::remove_dir_all(&base_dir);

        assert!(result.is_ok());
        let val = result.unwrap();
        assert_eq!(val["content"], "safe content");
    }

    #[tokio::test]
    async fn test_path_traversal_write() {
        let base_dir = std::env::current_dir().unwrap().join("test_base_write");
        fs::create_dir_all(&base_dir).unwrap();

        let tool = FileTool::new(base_dir.to_str().unwrap().to_string());
        let ctx = Arc::new(MockContext);

        let params = json!({
            "operation": "write",
            "path": "../traversal_write.txt",
            "content": "dangerous content"
        });

        let result = tool.execute(ctx, params).await;

        // Cleanup
        let _ = fs::remove_dir_all(&base_dir);
        let outside_file = std::env::current_dir().unwrap().join("traversal_write.txt");
        let _ = fs::remove_file(&outside_file);

        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("Path traversal attempt detected"));
    }

    #[tokio::test]
    async fn test_path_traversal_list() {
        let base_dir = std::env::current_dir().unwrap().join("test_base_list");
        fs::create_dir_all(&base_dir).unwrap();

        let tool = FileTool::new(base_dir.to_str().unwrap().to_string());
        let ctx = Arc::new(MockContext);

        let params = json!({
            "operation": "list",
            "path": ".."
        });

        let result = tool.execute(ctx, params).await;

        // Cleanup
        let _ = fs::remove_dir_all(&base_dir);

        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("Path traversal attempt detected"));
    }
}
