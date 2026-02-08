use std::sync::Arc;

use async_trait::async_trait;

use crate::safety::risk::{RiskTier, classify_prompt_risk};

use super::{
    bridge::{AppExecutionInput, run_app_agent},
    handoff::HandoffPolicyDecision,
    manifest::{AppManifest, default_manifests},
};

#[derive(Debug, Clone)]
pub struct IntentRoute {
    pub selected_apps: Vec<String>,
    pub risk: RiskTier,
    pub rationale: String,
}

#[derive(Debug, Clone)]
pub struct CommandDispatch {
    pub accepted: bool,
    pub summary: String,
}

#[async_trait]
pub trait AgentAppHost: Send + Sync {
    async fn list_apps(&self) -> Vec<AppManifest>;
    async fn route_prompt(&self, prompt: &str) -> IntentRoute;
    async fn execute_command(&self, app_id: &str, command: &str) -> CommandDispatch;
    async fn evaluate_handoff_policy(&self, from_app: &str, to_app: &str) -> HandoffPolicyDecision;
}

#[derive(Debug, Clone)]
pub struct InMemoryAgentHost {
    apps: Arc<Vec<AppManifest>>,
}

impl Default for InMemoryAgentHost {
    fn default() -> Self {
        Self {
            apps: Arc::new(default_manifests()),
        }
    }
}

#[async_trait]
impl AgentAppHost for InMemoryAgentHost {
    async fn list_apps(&self) -> Vec<AppManifest> {
        self.apps.as_ref().clone()
    }

    async fn route_prompt(&self, prompt: &str) -> IntentRoute {
        let p = prompt.to_lowercase();
        let risk = classify_prompt_risk(prompt);
        let tokens: Vec<String> = p
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|item| !item.is_empty())
            .map(ToString::to_string)
            .collect();

        let mut scored: Vec<(String, i32)> = self
            .apps
            .iter()
            .map(|app| {
                let mut score = 0;
                let haystack = format!(
                    "{} {} {}",
                    app.name.to_lowercase(),
                    app.description.to_lowercase(),
                    app.capabilities.join(" ").to_lowercase(),
                );
                for token in &tokens {
                    if haystack.contains(token) {
                        score += 3;
                    }
                    match token.as_str() {
                        "incident" | "service" | "rollback" | "restart" | "ops" if app.id == "ops-center" => score += 5,
                        "mail" | "email" | "inbox" if app.id == "mail-agent" => score += 5,
                        "calendar" | "meeting" | "schedule" if app.id == "calendar-agent" => score += 5,
                        _ => {}
                    }
                }
                (app.id.clone(), score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut selected: Vec<String> = scored
            .iter()
            .filter(|(_, score)| *score > 0)
            .take(2)
            .map(|(app_id, _)| app_id.clone())
            .collect();

        if selected.is_empty() {
            selected.push("ops-center".to_string());
        }

        let rationale = {
            let top = scored
                .into_iter()
                .filter(|(_, score)| *score > 0)
                .take(3)
                .map(|(app, score)| format!("{app}:{score}"))
                .collect::<Vec<_>>();
            if top.is_empty() {
                "fallback route selected: ops-center".to_string()
            } else {
                format!("capability score route selected ({})", top.join(", "))
            }
        };

        IntentRoute {
            selected_apps: selected,
            risk,
            rationale,
        }
    }

    async fn execute_command(&self, app_id: &str, command: &str) -> CommandDispatch {
        let app_exists = self.apps.iter().any(|app| app.id == app_id);
        if !app_exists {
            return CommandDispatch {
                accepted: false,
                summary: format!("Unknown app: {app_id}"),
            };
        }

        let output = run_app_agent(AppExecutionInput {
            app_id: app_id.to_string(),
            prompt: command.to_string(),
        })
        .await;
        CommandDispatch {
            accepted: true,
            summary: output.summary,
        }
    }

    async fn evaluate_handoff_policy(&self, from_app: &str, to_app: &str) -> HandoffPolicyDecision {
        let source_app = self.apps.iter().find(|app| app.id == from_app);
        if source_app.is_none() {
            return HandoffPolicyDecision {
                allowed: false,
                reason: format!("handoff blocked by policy: unknown source app `{from_app}`"),
            };
        }

        let target_exists = self.apps.iter().any(|app| app.id == to_app);
        if !target_exists {
            return HandoffPolicyDecision {
                allowed: false,
                reason: format!("handoff blocked by policy: unknown target app `{to_app}`"),
            };
        }

        if from_app == to_app {
            return HandoffPolicyDecision {
                allowed: true,
                reason: "handoff allowed by policy: intra-app transfer".to_string(),
            };
        }

        let source_app = source_app.expect("checked above");
        if source_app
            .handoff_allowlist
            .iter()
            .any(|allowed| allowed == to_app)
        {
            return HandoffPolicyDecision {
                allowed: true,
                reason: format!("handoff allowed by allowlist: {from_app} -> {to_app}"),
            };
        }

        HandoffPolicyDecision {
            allowed: false,
            reason: format!(
                "handoff blocked by allowlist: `{to_app}` is not allowed for `{from_app}`"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentAppHost, InMemoryAgentHost};

    #[tokio::test]
    async fn route_prompt_prefers_capability_scored_apps() {
        let host = InMemoryAgentHost::default();
        let route = host
            .route_prompt("schedule a meeting and fix calendar conflicts")
            .await;
        assert!(
            route.selected_apps.iter().any(|app| app == "calendar-agent"),
            "expected calendar-agent in selected apps"
        );
        assert!(
            route.rationale.contains("capability score route selected")
                || route.rationale.contains("fallback route selected"),
            "unexpected rationale format: {}",
            route.rationale
        );
    }

    #[tokio::test]
    async fn execute_command_rejects_unknown_app() {
        let host = InMemoryAgentHost::default();
        let dispatch = host.execute_command("unknown-app", "do something").await;
        assert!(!dispatch.accepted);
        assert!(dispatch.summary.contains("Unknown app"));
    }

    #[tokio::test]
    async fn handoff_policy_allows_listed_route() {
        let host = InMemoryAgentHost::default();
        let decision = host
            .evaluate_handoff_policy("ops-center", "mail-agent")
            .await;
        assert!(decision.allowed, "expected route to be allowed");
        assert!(decision.reason.contains("allowlist"));
    }

    #[tokio::test]
    async fn handoff_policy_blocks_unlisted_route() {
        let host = InMemoryAgentHost::default();
        let decision = host
            .evaluate_handoff_policy("mail-agent", "calendar-agent")
            .await;
        assert!(!decision.allowed, "expected route to be blocked");
        assert!(decision.reason.contains("blocked"));
    }
}
