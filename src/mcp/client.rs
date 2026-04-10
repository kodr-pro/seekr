use crate::mcp::types::*;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

pub struct McpClient {
    _child: Child,
    stdin: ChildStdin,
    pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    next_id: Arc<Mutex<u64>>,
    pub server_info: Option<Implementation>,
    pub capabilities: Option<ServerCapabilities>,
    pub notification_rx: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<JsonRpcNotification>>>,
}

impl McpClient {
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", command))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdout"))?;

        let pending_requests = Arc::new(Mutex::new(HashMap::new()));
        let (notif_tx, notif_rx) = tokio::sync::mpsc::unbounded_channel();
        let pending_clone = pending_requests.clone();

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            if let Err(e) = Self::read_loop(&mut reader, pending_clone, notif_tx).await {
                tracing::error!("MCP read loop error: {}", e);
            }
        });

        let mut client = Self {
            _child: child,
            stdin,
            pending_requests,
            next_id: Arc::new(Mutex::new(1)),
            server_info: None,
            capabilities: None,
            notification_rx: Arc::new(Mutex::new(notif_rx)),
        };

        client.initialize().await?;

        Ok(client)
    }

    async fn initialize(&mut self) -> Result<()> {
        let params = InitializeParams {
            protocol_version: "2024-11-05".to_string(), // Typical MCP version
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "Seekr".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let resp = self
            .request("initialize", serde_json::to_value(params)?)
            .await?;
        let result: InitializeResult = serde_json::from_value(resp)?;

        self.server_info = Some(result.server_info);
        self.capabilities = Some(result.capabilities);

        // Send initialized notification
        self.notify("notifications/initialized", json!({})).await?;

        Ok(())
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDefinition>> {
        let resp = self.request("tools/list", json!({})).await?;
        let result: ListToolsResult = serde_json::from_value(resp)?;
        Ok(result.tools)
    }

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let params = CallToolParams {
            name: name.to_string(),
            arguments,
        };
        let resp = self
            .request("tools/call", serde_json::to_value(params)?)
            .await?;
        let result: CallToolResult = serde_json::from_value(resp)?;
        Ok(result)
    }

    pub async fn list_resources(&mut self) -> Result<Vec<Resource>> {
        let resp = self.request("resources/list", json!({})).await?;
        let result: ListResourcesResult = serde_json::from_value(resp)?;
        Ok(result.resources)
    }

    pub async fn read_resource(&mut self, uri: &str) -> Result<ReadResourceResult> {
        let resp = self
            .request("resources/read", json!({ "uri": uri }))
            .await?;
        let result: ReadResourceResult = serde_json::from_value(resp)?;
        Ok(result)
    }

    pub async fn list_prompts(&mut self) -> Result<Vec<Prompt>> {
        let resp = self.request("prompts/list", json!({})).await?;
        let result: ListPromptsResult = serde_json::from_value(resp)?;
        Ok(result.prompts)
    }

    pub async fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<GetPromptResult> {
        let resp = self
            .request(
                "prompts/get",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        let result: GetPromptResult = serde_json::from_value(resp)?;
        Ok(result)
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = {
            let mut id_guard = self.next_id.lock().await;
            let id = *id_guard;
            *id_guard += 1;
            id
        };

        let (tx, rx) = oneshot::channel();
        self.pending_requests.lock().await.insert(id, tx);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };

        let body = serde_json::to_string(&req)?;
        self.send_raw(&body).await?;

        rx.await.context("MCP request channel closed")?
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };

        let body = serde_json::to_string(&notif)?;
        self.send_raw(&body).await?;

        Ok(())
    }

    async fn send_raw(&mut self, body: &str) -> Result<()> {
        // MCP over stdio doesn't use Content-Length headers like LSP by default!
        // It's usually one JSON-RPC message per line.
        self.stdin.write_all(body.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_loop(
        reader: &mut BufReader<tokio::process::ChildStdout>,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
        notif_tx: tokio::sync::mpsc::UnboundedSender<JsonRpcNotification>,
    ) -> Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }

            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&line) {
                if let Some(id) = resp.id {
                    let mut guard = pending.lock().await;
                    if let Some(tx) = guard.remove(&id) {
                        if let Some(error) = resp.error {
                            tx.send(Err(anyhow!(
                                "MCP Error ({}): {}",
                                error.code,
                                error.message
                            )))
                            .ok();
                        } else {
                            tx.send(Ok(resp.result.unwrap_or(Value::Null))).ok();
                        }
                    }
                }
            } else if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&line) {
                notif_tx.send(notif).ok();
            }
        }
        Ok(())
    }
}
