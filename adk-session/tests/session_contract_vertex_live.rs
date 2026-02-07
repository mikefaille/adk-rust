#![cfg(feature = "vertex-session")]

mod common;

use adk_session::{VertexAiSessionConfig, VertexAiSessionService};
use uuid::Uuid;

const ENV_PROJECT_ID: &str = "GOOGLE_PROJECT_ID";
const ENV_LOCATION: &str = "GOOGLE_CLOUD_LOCATION";
const ENV_APP_NAME: &str = "ADK_VERTEX_SESSION_APP_NAME";
const ENV_OTHER_APP_NAME: &str = "ADK_VERTEX_SESSION_OTHER_APP_NAME";

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "{name} is required for live Vertex session contract test. \
set {ENV_PROJECT_ID}, {ENV_LOCATION}, {ENV_APP_NAME}, and {ENV_OTHER_APP_NAME}."
        )
    })
}

#[tokio::test]
#[ignore = "requires live Vertex Session Service resources + ADC; run with --ignored"]
async fn test_vertex_service_live_contract() {
    let project_id = required_env(ENV_PROJECT_ID);
    let location = required_env(ENV_LOCATION);
    let app_name = required_env(ENV_APP_NAME);
    let other_app_name = required_env(ENV_OTHER_APP_NAME);

    let service =
        VertexAiSessionService::new_with_adc(VertexAiSessionConfig::new(project_id, location))
            .expect("build vertex session service");

    let run_id = Uuid::new_v4().simple().to_string();
    let user_1 = format!("adk-rust-live-u1-{run_id}");
    let user_2 = format!("adk-rust-live-u2-{run_id}");

    common::session_contract::assert_session_contract_with_users(
        &service,
        &app_name,
        &other_app_name,
        &user_1,
        &user_2,
    )
    .await;
}
