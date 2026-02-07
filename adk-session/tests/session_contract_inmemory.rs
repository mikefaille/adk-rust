mod common;

use adk_session::InMemorySessionService;

#[tokio::test]
async fn test_inmemory_service_contract() {
    let service = InMemorySessionService::new();
    common::session_contract::assert_session_contract(&service, "contract_app", "contract_app_2")
        .await;
}
