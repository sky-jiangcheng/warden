// A one-off probe against the REAL repotory-mcp on this machine.
//
// Not a test: it needs the Cinder desktop app installed, so it cannot be
// one. It is the counterpart of the fake-server test — the same client
// code, the real server.
//
// Run it with: cargo run --example real_mcp_probe

use repotory_agent_lib::mcp_client::{McpClient, McpSession};
use repotory_agent_lib::memory_layer;

#[tokio::main]
async fn main() {
    let binary = std::path::PathBuf::from("/usr/local/bin/repotory-mcp");
    if !binary.exists() {
        println!("repotory-mcp is not installed here");
        return;
    }

    // The payload the first screen actually receives: the same function the
    // `shell_status` command calls, against the same real server.
    let session = McpSession::default();
    let status = memory_layer::status(&session).await;
    println!(
        "{}",
        serde_json::to_string_pretty(&status).expect("serialize the status")
    );

    println!("\n--- tools/call repotory_projects_list ---");
    let (client, _) = McpClient::connect(&binary).await.expect("the handshake");
    let text = client
        .call_tool("repotory_projects_list", serde_json::json!({}))
        .await
        .expect("the tool answers");
    println!("{}", text.chars().take(300).collect::<String>());

    println!("\n--- tools/call no_such_tool (must be an error) ---");
    match client
        .call_tool("no_such_tool", serde_json::json!({}))
        .await
    {
        Ok(text) => println!("UNEXPECTED OK: {text}"),
        Err(err) => println!("error reached the caller: {err}"),
    }
}
