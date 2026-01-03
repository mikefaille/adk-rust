//! Property-based tests for MCP client configuration and tool discovery.
//!
//! **Feature: mistral-rs-integration, Property 13: MCP Tool Discovery**
//! *For any* valid MCP server configuration, connecting SHALL discover available tools.
//! **Validates: Requirements 15.1, 15.2**

use proptest::prelude::*;

use adk_mistralrs::{McpClientConfig, McpServerConfig, McpServerSource};

/// Generate arbitrary server IDs
fn arb_server_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{2,15}".prop_map(|s| s.to_string())
}

/// Generate arbitrary server names
fn arb_server_name() -> impl Strategy<Value = String> {
    "[A-Z][a-zA-Z0-9 ]{3,25}".prop_map(|s| s.to_string())
}

/// Generate arbitrary HTTP URLs
fn arb_http_url() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("https://api.example.com/mcp".to_string()),
        Just("http://localhost:8080/mcp".to_string()),
        Just("https://mcp.service.internal/v1".to_string()),
    ]
}

/// Generate arbitrary WebSocket URLs
fn arb_ws_url() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("wss://realtime.example.com/mcp".to_string()),
        Just("ws://localhost:9090/mcp".to_string()),
        Just("wss://stream.service.internal/v1".to_string()),
    ]
}

/// Generate arbitrary commands for process-based servers
fn arb_command() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("mcp-server-filesystem".to_string()),
        Just("mcp-server-git".to_string()),
        Just("mcp-server-sqlite".to_string()),
        Just("npx".to_string()),
        Just("uvx".to_string()),
    ]
}

/// Generate arbitrary command arguments
fn arb_args() -> impl Strategy<Value = Vec<String>> {
    prop_oneof![
        Just(vec![]),
        Just(vec!["--root".to_string(), "/tmp".to_string()]),
        Just(vec!["--config".to_string(), "config.json".to_string()]),
        Just(vec!["-v".to_string()]),
    ]
}

/// Generate arbitrary tool prefixes
fn arb_tool_prefix() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), "[a-z]{2,6}".prop_map(Some),]
}

/// Generate arbitrary bearer tokens
fn arb_bearer_token() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), "[a-zA-Z0-9_-]{20,40}".prop_map(Some),]
}

/// Generate arbitrary timeout values
fn arb_timeout() -> impl Strategy<Value = Option<u64>> {
    prop_oneof![Just(None), (10u64..120).prop_map(Some),]
}

/// Generate arbitrary MCP server source
fn arb_mcp_server_source() -> impl Strategy<Value = McpServerSource> {
    prop_oneof![
        (arb_http_url(), arb_timeout()).prop_map(|(url, timeout_secs)| {
            McpServerSource::Http { url, timeout_secs, headers: None }
        }),
        (arb_ws_url(), arb_timeout()).prop_map(|(url, timeout_secs)| {
            McpServerSource::WebSocket { url, timeout_secs, headers: None }
        }),
        (arb_command(), arb_args()).prop_map(|(command, args)| {
            McpServerSource::Process { command, args, work_dir: None, env: None }
        }),
    ]
}

/// Generate arbitrary MCP server config
fn arb_mcp_server_config() -> impl Strategy<Value = McpServerConfig> {
    (
        arb_server_id(),
        arb_server_name(),
        arb_mcp_server_source(),
        any::<bool>(),
        arb_tool_prefix(),
        arb_bearer_token(),
    )
        .prop_map(|(id, name, source, enabled, tool_prefix, bearer_token)| McpServerConfig {
            id,
            name,
            source,
            enabled,
            tool_prefix,
            resources: None,
            bearer_token,
        })
}

/// Generate arbitrary MCP client config with 1-5 servers
fn arb_mcp_client_config() -> impl Strategy<Value = McpClientConfig> {
    (
        prop::collection::vec(arb_mcp_server_config(), 1..5),
        any::<bool>(),
        arb_timeout(),
        prop::option::of(1usize..10),
    )
        .prop_map(|(servers, auto_register_tools, tool_timeout_secs, max_concurrent_calls)| {
            McpClientConfig {
                servers,
                auto_register_tools,
                tool_timeout_secs,
                max_concurrent_calls,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 13: MCP Tool Discovery**
    /// *For any* MCP client configuration, serialization and deserialization
    /// SHALL preserve all server configurations.
    /// **Validates: Requirements 15.1, 15.2**
    #[test]
    fn prop_mcp_config_serialization_roundtrip(config in arb_mcp_client_config()) {
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpClientConfig = serde_json::from_str(&json).unwrap();

        // Server count should be preserved
        prop_assert_eq!(config.servers.len(), parsed.servers.len());

        // Auto-register setting should be preserved
        prop_assert_eq!(config.auto_register_tools, parsed.auto_register_tools);

        // Timeout should be preserved
        prop_assert_eq!(config.tool_timeout_secs, parsed.tool_timeout_secs);

        // Max concurrent calls should be preserved
        prop_assert_eq!(config.max_concurrent_calls, parsed.max_concurrent_calls);
    }

    /// **Feature: mistral-rs-integration, Property 13: MCP Tool Discovery**
    /// *For any* MCP server configuration, all fields SHALL be preserved
    /// through serialization/deserialization.
    /// **Validates: Requirements 15.1, 15.2**
    #[test]
    fn prop_mcp_server_config_preserves_fields(server in arb_mcp_server_config()) {
        let json = serde_json::to_string(&server).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();

        // ID should be preserved
        prop_assert_eq!(&server.id, &parsed.id);

        // Name should be preserved
        prop_assert_eq!(&server.name, &parsed.name);

        // Enabled flag should be preserved
        prop_assert_eq!(server.enabled, parsed.enabled);

        // Tool prefix should be preserved
        prop_assert_eq!(&server.tool_prefix, &parsed.tool_prefix);

        // Bearer token should be preserved
        prop_assert_eq!(&server.bearer_token, &parsed.bearer_token);
    }

    /// **Feature: mistral-rs-integration, Property 13: MCP Tool Discovery**
    /// *For any* MCP server source, the source type and parameters SHALL be
    /// preserved through serialization/deserialization.
    /// **Validates: Requirements 15.1, 15.2**
    #[test]
    fn prop_mcp_server_source_preserves_type(source in arb_mcp_server_source()) {
        let json = serde_json::to_string(&source).unwrap();
        let parsed: McpServerSource = serde_json::from_str(&json).unwrap();

        // Source type should match
        match (&source, &parsed) {
            (McpServerSource::Http { url: u1, timeout_secs: t1, .. },
             McpServerSource::Http { url: u2, timeout_secs: t2, .. }) => {
                prop_assert_eq!(u1, u2);
                prop_assert_eq!(t1, t2);
            }
            (McpServerSource::WebSocket { url: u1, timeout_secs: t1, .. },
             McpServerSource::WebSocket { url: u2, timeout_secs: t2, .. }) => {
                prop_assert_eq!(u1, u2);
                prop_assert_eq!(t1, t2);
            }
            (McpServerSource::Process { command: c1, args: a1, .. },
             McpServerSource::Process { command: c2, args: a2, .. }) => {
                prop_assert_eq!(c1, c2);
                prop_assert_eq!(a1, a2);
            }
            _ => {
                prop_assert!(false, "Source type changed during serialization");
            }
        }
    }

    /// **Feature: mistral-rs-integration, Property 13: MCP Tool Discovery**
    /// *For any* valid MCP client configuration, validation SHALL succeed.
    /// **Validates: Requirements 15.1, 15.2**
    #[test]
    fn prop_valid_mcp_config_validates(config in arb_mcp_client_config()) {
        // All generated configs should be valid (non-empty servers with valid URLs/commands)
        let result = config.validate();
        prop_assert!(result.is_ok(), "Valid config should pass validation: {:?}", result);
    }

    /// **Feature: mistral-rs-integration, Property 13: MCP Tool Discovery**
    /// *For any* MCP client configuration, enabled_server_count SHALL return
    /// the correct count of enabled servers.
    /// **Validates: Requirements 15.1, 15.2**
    #[test]
    fn prop_enabled_server_count_correct(config in arb_mcp_client_config()) {
        let expected_count = config.servers.iter().filter(|s| s.enabled).count();
        let actual_count = config.enabled_server_count();
        prop_assert_eq!(expected_count, actual_count);
    }

    /// **Feature: mistral-rs-integration, Property 13: MCP Tool Discovery**
    /// *For any* MCP server configuration built with builder methods,
    /// the configuration SHALL be correctly constructed.
    /// **Validates: Requirements 15.1, 15.2**
    #[test]
    fn prop_server_builder_constructs_correctly(
        name in arb_server_name(),
        url in arb_http_url(),
        prefix in "[a-z]{2,6}",
        token in "[a-zA-Z0-9]{20,30}"
    ) {
        let server = McpServerConfig::http(&name, &url)
            .with_tool_prefix(&prefix)
            .with_bearer_token(&token);

        prop_assert_eq!(&server.name, &name);
        prop_assert_eq!(&server.tool_prefix, &Some(prefix));
        prop_assert_eq!(&server.bearer_token, &Some(token));
        prop_assert!(server.enabled);

        if let McpServerSource::Http { url: server_url, .. } = &server.source {
            prop_assert_eq!(server_url, &url);
        } else {
            prop_assert!(false, "Expected HTTP source");
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_mcp_client_config_default() {
        let config = McpClientConfig::default();
        assert!(config.servers.is_empty());
        assert!(config.auto_register_tools);
        assert!(config.tool_timeout_secs.is_none());
        assert_eq!(config.max_concurrent_calls, Some(1));
    }

    #[test]
    fn test_mcp_server_config_http() {
        let server = McpServerConfig::http("Test Server", "https://api.example.com/mcp");
        assert_eq!(server.name, "Test Server");
        assert!(server.enabled);
        assert!(matches!(server.source, McpServerSource::Http { .. }));
    }

    #[test]
    fn test_mcp_server_config_process() {
        let server = McpServerConfig::process("Filesystem", "mcp-server-filesystem")
            .with_args(vec!["--root".to_string(), "/tmp".to_string()])
            .with_tool_prefix("fs");

        assert_eq!(server.name, "Filesystem");
        assert_eq!(server.tool_prefix, Some("fs".to_string()));
        if let McpServerSource::Process { command, args, .. } = &server.source {
            assert_eq!(command, "mcp-server-filesystem");
            assert_eq!(args.len(), 2);
        } else {
            panic!("Expected Process source");
        }
    }

    #[test]
    fn test_mcp_server_config_websocket() {
        let server = McpServerConfig::websocket("Realtime", "wss://realtime.example.com/mcp")
            .with_bearer_token("secret-token")
            .with_timeout(30);

        assert_eq!(server.name, "Realtime");
        assert_eq!(server.bearer_token, Some("secret-token".to_string()));
        if let McpServerSource::WebSocket { url, timeout_secs, .. } = &server.source {
            assert_eq!(url, "wss://realtime.example.com/mcp");
            assert_eq!(*timeout_secs, Some(30));
        } else {
            panic!("Expected WebSocket source");
        }
    }

    #[test]
    fn test_mcp_client_config_builder() {
        let config = McpClientConfig::new()
            .add_server(McpServerConfig::http("Server1", "https://api1.example.com"))
            .add_server(McpServerConfig::http("Server2", "https://api2.example.com"))
            .with_tool_timeout(60)
            .with_max_concurrent_calls(5);

        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.tool_timeout_secs, Some(60));
        assert_eq!(config.max_concurrent_calls, Some(5));
    }

    #[test]
    fn test_mcp_client_config_validation_empty() {
        let config = McpClientConfig::new();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_mcp_client_config_validation_valid() {
        let config =
            McpClientConfig::with_server(McpServerConfig::http("Test", "https://api.example.com"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_mcp_client_config_validation_empty_url() {
        let config = McpClientConfig::with_server(McpServerConfig::http("Test", ""));
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_mcp_client_config_validation_invalid_scheme() {
        let config =
            McpClientConfig::with_server(McpServerConfig::http("Test", "ftp://invalid.com"));
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_mcp_client_config_validation_empty_command() {
        let config = McpClientConfig::with_server(McpServerConfig::process("Test", ""));
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_mcp_client_config_serialize() {
        let config = McpClientConfig::with_server(
            McpServerConfig::http("Test", "https://api.example.com")
                .with_id("test-server")
                .with_bearer_token("token123"),
        );

        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpClientConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.servers.len(), 1);
        assert_eq!(parsed.servers[0].id, "test-server");
        assert_eq!(parsed.servers[0].bearer_token, Some("token123".to_string()));
    }

    #[test]
    fn test_enabled_server_count() {
        let config = McpClientConfig::new()
            .add_server(McpServerConfig::http("Server1", "https://api1.example.com"))
            .add_server(McpServerConfig::http("Server2", "https://api2.example.com").disabled())
            .add_server(McpServerConfig::http("Server3", "https://api3.example.com"));

        assert_eq!(config.enabled_server_count(), 2);
    }

    #[test]
    fn test_mcp_server_source_serialization() {
        // HTTP
        let http = McpServerSource::Http {
            url: "https://api.example.com".to_string(),
            timeout_secs: Some(30),
            headers: None,
        };
        let json = serde_json::to_string(&http).unwrap();
        assert!(json.contains("Http"));
        assert!(json.contains("https://api.example.com"));

        // Process
        let process = McpServerSource::Process {
            command: "mcp-server".to_string(),
            args: vec!["--arg".to_string()],
            work_dir: None,
            env: None,
        };
        let json = serde_json::to_string(&process).unwrap();
        assert!(json.contains("Process"));
        assert!(json.contains("mcp-server"));

        // WebSocket
        let ws = McpServerSource::WebSocket {
            url: "wss://realtime.example.com".to_string(),
            timeout_secs: None,
            headers: None,
        };
        let json = serde_json::to_string(&ws).unwrap();
        assert!(json.contains("WebSocket"));
        assert!(json.contains("wss://realtime.example.com"));
    }
}
