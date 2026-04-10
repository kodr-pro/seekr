use anyhow::{Context, Result, anyhow};
use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcNotification {
    jsonrpc: String,
    method: String,
    params: Value,
}

pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    next_id: Arc<Mutex<u64>>,
}

impl LspClient {
    pub async fn spawn(command: &str, args: &[&str], root_path: &Path) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("Failed to spawn LSP server: {}", command))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdout"))?;

        let pending_requests = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending_requests.clone();

        // Spawn stdout reader loop
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            if let Err(e) = Self::read_loop(&mut reader, pending_clone).await {
                tracing::error!("LSP read loop error: {}", e);
            }
        });

        let root_uri = Uri::from_str(&format!("file://{}", root_path.to_string_lossy()))
            .map_err(|_| anyhow!("Failed to convert root path to URI"))?;

        let mut client = Self {
            child,
            stdin,
            pending_requests,
            next_id: Arc::new(Mutex::new(1)),
        };

        client.initialize(root_uri).await?;

        Ok(client)
    }

    async fn initialize(&mut self, root_uri: Uri) -> Result<()> {
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: "seekr_workspace".to_string(),
            }]),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    definition: Some(GotoCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(false),
                    }),
                    references: Some(ReferenceClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    hover: Some(HoverClientCapabilities {
                        dynamic_registration: Some(false),
                        content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let _response = self
            .request("initialize", serde_json::to_value(params)?)
            .await?;

        // Send initialized notification
        self.notify("initialized", json!({})).await?;

        Ok(())
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

        rx.await.context("LSP request channel closed")?
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
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(body.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_loop(
        reader: &mut BufReader<tokio::process::ChildStdout>,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    ) -> Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }

            if line.starts_with("Content-Length:") {
                let len_str = line.trim_start_matches("Content-Length:").trim();
                let len: usize = len_str.parse().context("Invalid Content-Length")?;

                // Skip the \r\n after header and the \r\n separator
                let mut buffer = String::new();
                while reader.read_line(&mut buffer).await? != 0 {
                    if buffer == "\r\n" {
                        break;
                    }
                    buffer.clear();
                }

                let mut body_buf = vec![0u8; len];
                reader.read_exact(&mut body_buf).await?;
                let body = String::from_utf8(body_buf).context("LSP body not UTF-8")?;

                if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&body)
                    && let Some(id_val) = resp.id
                    && let Some(id) = id_val.as_u64()
                    && let Some(tx) = pending.lock().await.remove(&id)
                {
                    if let Some(error) = resp.error {
                        tx.send(Err(anyhow!("LSP Error: {}", error))).ok();
                    } else {
                        tx.send(Ok(resp.result.unwrap_or(Value::Null))).ok();
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn goto_definition(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Value> {
        let uri = Uri::from_str(&format!("file://{}", path.to_string_lossy()))
            .map_err(|_| anyhow!("Invalid file path"))?;
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("textDocument/definition", serde_json::to_value(params)?)
            .await
    }

    pub async fn find_references(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Value> {
        let uri = Uri::from_str(&format!("file://{}", path.to_string_lossy()))
            .map_err(|_| anyhow!("Invalid file path"))?;
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: ReferenceContext {
                include_declaration: true,
            },
        };

        self.request("textDocument/references", serde_json::to_value(params)?)
            .await
    }

    pub async fn hover(&mut self, path: &Path, line: u32, character: u32) -> Result<Value> {
        let uri = Uri::from_str(&format!("file://{}", path.to_string_lossy()))
            .map_err(|_| anyhow!("Invalid file path"))?;
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
        };

        self.request("textDocument/hover", serde_json::to_value(params)?)
            .await
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // We can't await in drop, but we can try to send shutdown.
        // In practice, the child will be killed when the task ends if configured,
        // or we rely on the OS.
        let _ = self.child.start_kill();
    }
}
