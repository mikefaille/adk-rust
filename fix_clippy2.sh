sed -i 's/if config.rendering.is_some() {/#[allow(clippy::collapsible_if)]\n        if config.rendering.is_some() {/' adk-realtime/tests/avatar_property_tests.rs
