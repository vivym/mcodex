#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    codex_rmcp_client::run_test_stdio_server().await
}
