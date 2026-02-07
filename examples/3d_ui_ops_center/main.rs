use adk_3d_ui::{ServerConfig, run_server};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let host = std::env::var("ADK_3D_UI_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("ADK_3D_UI_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8099);

    println!("3D UI Ops Center running at http://{}:{}", host, port);
    run_server(ServerConfig { host, port }).await
}
