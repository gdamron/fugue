//! Development proxy for fugue-mcp with hot-reload.
//!
//! Sits between Claude Code and the real fugue-mcp process, proxying JSON-RPC
//! messages over stdio. Watches source files for changes and automatically
//! rebuilds and restarts the inner process, restoring invention state.
//!
//! Usage:
//!   cargo run --features dev --bin fugue-mcp-dev
//!
//! Or configure in .mcp.json as the dev server.

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// JSON-RPC helpers (minimal, no full MCP dependency needed)
// ---------------------------------------------------------------------------

/// Counter for internal request IDs (negative to avoid collision with client IDs).
static INTERNAL_ID: AtomicI64 = AtomicI64::new(-1);

fn next_internal_id() -> i64 {
    INTERNAL_ID.fetch_sub(1, Ordering::Relaxed)
}

fn make_tool_call(name: &str, arguments: serde_json::Value) -> (i64, String) {
    let id = next_internal_id();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": arguments,
        }
    });
    (id, serde_json::to_string(&request).unwrap())
}

fn make_initialize(original: &serde_json::Value) -> (i64, String) {
    let id = next_internal_id();
    let mut request = original.clone();
    request["id"] = serde_json::json!(id);
    (id, serde_json::to_string(&request).unwrap())
}

fn make_initialized_notification() -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    }))
    .unwrap()
}

fn extract_text_content(response: &serde_json::Value) -> Option<String> {
    let content = response.get("result")?.get("content")?.as_array()?;
    for item in content {
        if item.get("type")?.as_str()? == "text" {
            return item.get("text").and_then(|t| t.as_str()).map(String::from);
        }
    }
    None
}

fn make_error_response(id: &serde_json::Value, message: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32603,
            "message": message,
        }
    }))
    .unwrap()
}

// ---------------------------------------------------------------------------
// Child process management
// ---------------------------------------------------------------------------

struct McpChild {
    process: Child,
    stdin: tokio::io::BufWriter<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl McpChild {
    async fn spawn() -> Result<Self, Box<dyn std::error::Error>> {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let mut process = Command::new(cargo)
            .args(["run", "--release", "--features", "mcp", "--bin", "fugue-mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // Build/runtime output visible to developer
            .kill_on_drop(true)
            .spawn()?;

        let stdin = tokio::io::BufWriter::new(process.stdin.take().unwrap());
        let stdout = BufReader::new(process.stdout.take().unwrap());

        Ok(Self {
            process,
            stdin,
            stdout,
        })
    }

    async fn send(&mut self, line: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.stdin.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            self.stdin.write_all(b"\n").await?;
        }
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(line))
    }

    /// Send a request and wait for the response with the matching ID.
    /// Non-matching messages (notifications, other responses) are collected and returned.
    async fn call(
        &mut self,
        id: i64,
        request: &str,
    ) -> Result<(serde_json::Value, Vec<String>), Box<dyn std::error::Error>> {
        self.send(request).await?;
        let mut other_messages = Vec::new();

        loop {
            let line = self
                .read_line()
                .await?
                .ok_or("Child process exited unexpectedly")?;

            let parsed: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Check if this is the response to our request
            if let Some(resp_id) = parsed.get("id") {
                if resp_id.as_i64() == Some(id) {
                    return Ok((parsed, other_messages));
                }
            }

            // Not our response — save for later
            other_messages.push(line);
        }
    }

    async fn kill(&mut self) {
        let _ = self.process.kill().await;
        let _ = self.process.wait().await;
    }
}

// ---------------------------------------------------------------------------
// State capture and restore
// ---------------------------------------------------------------------------

/// Captures the current invention state from the running child via MCP tool calls.
/// Returns the invention JSON string if an invention is running, None otherwise.
async fn capture_state(child: &mut McpChild) -> Option<String> {
    // Check status
    let (id, req) = make_tool_call("get_status", serde_json::json!({}));
    let (resp, _) = child.call(id, &req).await.ok()?;
    let status_text = extract_text_content(&resp)?;
    let status: serde_json::Value = serde_json::from_str(&status_text).ok()?;

    if !status.get("running")?.as_bool()? {
        return None;
    }

    // Get modules
    let (id, req) = make_tool_call("list_modules", serde_json::json!({}));
    let (resp, _) = child.call(id, &req).await.ok()?;
    let modules_text = extract_text_content(&resp)?;
    let modules: Vec<serde_json::Value> = serde_json::from_str(&modules_text).ok()?;

    // Get connections
    let (id, req) = make_tool_call("list_connections", serde_json::json!({}));
    let (resp, _) = child.call(id, &req).await.ok()?;
    let connections_text = extract_text_content(&resp)?;
    let connections: Vec<serde_json::Value> = serde_json::from_str(&connections_text).ok()?;

    // Build invention JSON
    let invention = serde_json::json!({
        "version": "1.0.0",
        "modules": modules.iter().map(|m| {
            serde_json::json!({
                "id": m["id"],
                "module_type": m["module_type"],
                "config": m.get("config").cloned().unwrap_or(serde_json::Value::Null),
            })
        }).collect::<Vec<_>>(),
        "connections": connections.iter().map(|c| {
            serde_json::json!({
                "from": c["from"],
                "to": c["to"],
                "from_port": c["from_port"],
                "to_port": c["to_port"],
            })
        }).collect::<Vec<_>>(),
    });

    Some(serde_json::to_string(&invention).unwrap())
}

/// Restores invention state on the new child by calling load_invention.
async fn restore_state(
    child: &mut McpChild,
    invention_json: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (id, req) = make_tool_call(
        "load_invention",
        serde_json::json!({ "json": invention_json }),
    );
    let (resp, _) = child.call(id, &req).await?;

    if resp.get("error").is_some() {
        let err_msg = resp["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        eprintln!("[dev] Warning: state restore failed: {}", err_msg);
    } else {
        let text = extract_text_content(&resp).unwrap_or_default();
        eprintln!("[dev] State restored: {}", text);
    }
    Ok(())
}

/// Replay MCP initialization handshake on a new child.
/// Sends the original initialize request and initialized notification.
/// Returns any non-response messages the child sent during init.
async fn replay_init(
    child: &mut McpChild,
    original_init: &serde_json::Value,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (id, req) = make_initialize(original_init);
    let (_resp, extra) = child.call(id, &req).await?;

    // Send initialized notification
    child.send(&make_initialized_notification()).await?;

    Ok(extra)
}

// ---------------------------------------------------------------------------
// File watcher
// ---------------------------------------------------------------------------

fn setup_watcher(
    tx: mpsc::UnboundedSender<()>,
) -> Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>, Box<dyn std::error::Error>>
{
    let debouncer = new_debouncer(Duration::from_millis(500), move |events: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
        if let Ok(events) = events {
            let dominated_by_close_write = events.iter().any(|e| {
                matches!(e.kind, DebouncedEventKind::Any | DebouncedEventKind::AnyContinuous)
            });
            if dominated_by_close_write {
                let _ = tx.send(());
            }
        }
    })?;

    Ok(debouncer)
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

async fn rebuild() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[dev] Rebuilding fugue-mcp...");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .args(["build", "--release", "--features", "mcp", "--bin", "fugue-mcp"])
        .stderr(Stdio::inherit())
        .stdout(Stdio::inherit())
        .status()
        .await?;

    if !status.success() {
        return Err(format!("Build failed with status: {}", status).into());
    }

    eprintln!("[dev] Build successful.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Main proxy loop
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    eprintln!("[dev] Starting fugue-mcp-dev proxy...");

    // File watcher
    let (watch_tx, mut watch_rx) = mpsc::unbounded_channel();
    let mut debouncer = setup_watcher(watch_tx)?;

    let src_path = Path::new("src");
    let cargo_path = Path::new("Cargo.toml");
    debouncer
        .watcher()
        .watch(src_path, notify::RecursiveMode::Recursive)?;
    debouncer
        .watcher()
        .watch(cargo_path, notify::RecursiveMode::NonRecursive)?;

    // Also watch examples and tests if they exist
    for dir in &["examples", "tests"] {
        let p = Path::new(dir);
        if p.exists() {
            let _ = debouncer.watcher().watch(p, notify::RecursiveMode::Recursive);
        }
    }

    // Spawn the child MCP server
    eprintln!("[dev] Spawning fugue-mcp...");
    let mut child = McpChild::spawn().await?;

    // State tracking
    let mut init_request: Option<serde_json::Value> = None;
    let mut pending_requests: Vec<(serde_json::Value, String)> = Vec::new(); // (id, raw_line)

    // Client I/O
    let client_stdin = tokio::io::stdin();
    let mut client_stdout = tokio::io::BufWriter::new(tokio::io::stdout());
    let mut client_reader = BufReader::new(client_stdin);

    let mut client_line = String::new();
    let mut child_line = String::new();

    loop {
        client_line.clear();
        child_line.clear();

        tokio::select! {
            // Client -> Child
            result = client_reader.read_line(&mut client_line) => {
                let n = result?;
                if n == 0 {
                    // Client disconnected
                    eprintln!("[dev] Client disconnected, shutting down.");
                    child.kill().await;
                    break;
                }

                let trimmed = client_line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Parse to inspect the message
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    // Capture initialize request for replay on restart
                    if parsed.get("method").and_then(|m| m.as_str()) == Some("initialize") {
                        init_request = Some(parsed.clone());
                    }

                    // Track pending requests (has id + method = request)
                    if let Some(id) = parsed.get("id") {
                        if parsed.get("method").is_some() {
                            pending_requests.push((id.clone(), client_line.clone()));
                        }
                    }
                }

                // Forward to child
                child.send(&client_line).await?;
            }

            // Child -> Client
            result = child.stdout.read_line(&mut child_line) => {
                let n = result?;
                if n == 0 {
                    // Child process exited unexpectedly
                    eprintln!("[dev] Child process exited unexpectedly.");
                    // If we have init info, try to restart
                    if init_request.is_some() {
                        eprintln!("[dev] Attempting restart...");
                        if let Err(e) = rebuild().await {
                            eprintln!("[dev] Rebuild failed: {}. Waiting for next file change.", e);
                            // Wait for a file change to retry
                            watch_rx.recv().await;
                            // Drain any extra events
                            while watch_rx.try_recv().is_ok() {}
                            if let Err(e) = rebuild().await {
                                eprintln!("[dev] Rebuild still failing: {}", e);
                                continue;
                            }
                        }
                        child = McpChild::spawn().await?;
                        if let Some(ref init) = init_request {
                            if let Err(e) = replay_init(&mut child, init).await {
                                eprintln!("[dev] Init replay failed: {}", e);
                            }
                        }
                        continue;
                    }
                    break;
                }

                let trimmed = child_line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Remove from pending if this is a response
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    if let Some(id) = parsed.get("id") {
                        if parsed.get("result").is_some() || parsed.get("error").is_some() {
                            pending_requests.retain(|(req_id, _)| req_id != id);
                        }
                    }
                }

                // Forward to client
                client_stdout.write_all(child_line.as_bytes()).await?;
                if !child_line.ends_with('\n') {
                    client_stdout.write_all(b"\n").await?;
                }
                client_stdout.flush().await?;
            }

            // File change detected
            _ = watch_rx.recv() => {
                // Drain any additional queued events
                while watch_rx.try_recv().is_ok() {}

                eprintln!("[dev] File change detected, hot-reloading...");

                // Capture current state before killing the child
                let state = if init_request.is_some() {
                    capture_state(&mut child).await
                } else {
                    None
                };

                // Send error responses for any pending requests
                for (id, _) in &pending_requests {
                    let err = make_error_response(id, "Server restarting due to code change");
                    client_stdout.write_all(err.as_bytes()).await?;
                    client_stdout.write_all(b"\n").await?;
                }
                client_stdout.flush().await?;
                pending_requests.clear();

                // Kill the old child
                child.kill().await;

                // Rebuild
                if let Err(e) = rebuild().await {
                    eprintln!("[dev] Build failed: {}. Waiting for next file change to retry.", e);
                    // Don't break — wait for the next file change to rebuild
                    loop {
                        watch_rx.recv().await;
                        while watch_rx.try_recv().is_ok() {}
                        eprintln!("[dev] File change detected, retrying build...");
                        match rebuild().await {
                            Ok(_) => break,
                            Err(e) => eprintln!("[dev] Build still failing: {}", e),
                        }
                    }
                }

                // Spawn new child
                child = McpChild::spawn().await?;

                // Replay initialization
                if let Some(ref init) = init_request {
                    match replay_init(&mut child, init).await {
                        Ok(_extra) => {
                            eprintln!("[dev] MCP re-initialized.");
                        }
                        Err(e) => {
                            eprintln!("[dev] Init replay failed: {}", e);
                        }
                    }
                }

                // Restore state
                if let Some(ref state_json) = state {
                    let _ = restore_state(&mut child, state_json).await;
                }

                eprintln!("[dev] Hot-reload complete.");
            }
        }
    }

    Ok(())
}
