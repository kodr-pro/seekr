use crate::agent::AgentEvent;
use reqwest::Client;
use tokio::sync::mpsc;

use super::server::{ChatMessageReq, NetworkEvent, ShellInputReq, StartAgentReq, ToolApprovalReq};

pub struct DaemonClient {
    http: Client,
    base_url: String,
    auth_token: Option<String>,
}

impl Default for DaemonClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonClient {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
            base_url: "http://127.0.0.1:8765".to_string(),
            auth_token: super::server::read_token_file(),
        }
    }

    fn auth_request(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref token) = self.auth_token {
            req.bearer_auth(token)
        } else {
            req
        }
    }

    pub async fn check_health(&self) -> bool {
        if let Ok(res) = self
            .http
            .get(format!("{}/health", self.base_url))
            .send()
            .await
        {
            res.status().is_success()
        } else {
            false
        }
    }

    pub async fn start_agent(&self, session_id: Option<String>) -> anyhow::Result<()> {
        let req = StartAgentReq { session_id };
        self.auth_request(
            self.http
                .post(format!("{}/start", self.base_url))
                .json(&req),
        )
        .send()
        .await?;
        Ok(())
    }

    pub async fn send_chat(&self, message: String) -> anyhow::Result<()> {
        let req = ChatMessageReq { message };
        self.auth_request(
            self.http
                .post(format!("{}/chat", self.base_url))
                .json(&req),
        )
        .send()
        .await?;
        Ok(())
    }

    pub async fn send_approval(&self, approved: bool, always: bool) -> anyhow::Result<()> {
        let req = ToolApprovalReq { approved, always };
        self.auth_request(
            self.http
                .post(format!("{}/command/approve", self.base_url))
                .json(&req),
        )
        .send()
        .await?;
        Ok(())
    }

    pub async fn send_shutdown(&self) -> anyhow::Result<()> {
        self.auth_request(
            self.http
                .post(format!("{}/command/shutdown", self.base_url)),
        )
        .send()
        .await?;
        Ok(())
    }

    pub async fn send_check_connection(&self) -> anyhow::Result<()> {
        self.auth_request(
            self.http
                .post(format!("{}/command/check_connection", self.base_url)),
        )
        .send()
        .await?;
        Ok(())
    }

    pub async fn send_shell_input(&self, input: String) -> anyhow::Result<()> {
        let req = ShellInputReq { input };
        self.auth_request(
            self.http
                .post(format!("{}/command/shell", self.base_url))
                .json(&req),
        )
        .send()
        .await?;
        Ok(())
    }

    pub async fn subscribe_events(
        &self,
        tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> anyhow::Result<()> {
        use futures_util::StreamExt;
        use reqwest_eventsource::{Event, EventSource};

        let url = format!("{}/events", self.base_url);
        let check_url = format!("{}/command/check_connection", self.base_url);
        let auth_token = self.auth_token.clone();

        let mut request = self.http.get(&url);
        if let Some(ref token) = auth_token {
            request = request.bearer_auth(token);
        }
        let mut es = EventSource::new(request)?;

        tokio::spawn(async move {
            while let Some(event) = es.next().await {
                match event {
                    Ok(Event::Open) => {
                        let dummy = Client::new();
                        let mut req = dummy.post(&check_url);
                        if let Some(ref token) = auth_token {
                            req = req.bearer_auth(token);
                        }
                        let _ = req.send().await;
                    }
                    Ok(Event::Message(message)) => {
                        let data = message.data.trim();
                        if data.is_empty() {
                            continue;
                        }
                        if let Ok(net_evt) = serde_json::from_str::<NetworkEvent>(data) {
                            let evt = match net_evt {
                                NetworkEvent::ContentDelta(s) => AgentEvent::ContentDelta(s),
                                NetworkEvent::ReasoningDelta(s) => AgentEvent::ReasoningDelta(s),
                                NetworkEvent::ToolCallStart { name, arguments } => {
                                    AgentEvent::ToolCallStart { name, arguments }
                                }
                                NetworkEvent::ToolCallResult { name, result } => {
                                    AgentEvent::ToolCallResult { name, result }
                                }
                                NetworkEvent::Activity(a) => AgentEvent::Activity(a),
                                NetworkEvent::TokenUsage {
                                    prompt_tokens,
                                    completion_tokens,
                                    total_tokens,
                                } => AgentEvent::TokenUsage {
                                    prompt_tokens,
                                    completion_tokens,
                                    total_tokens,
                                },
                                NetworkEvent::IterationUpdate(n) => AgentEvent::IterationUpdate(n),
                                NetworkEvent::TurnComplete => AgentEvent::TurnComplete,
                                NetworkEvent::MaxIterationsReached => {
                                    AgentEvent::MaxIterationsReached
                                }
                                NetworkEvent::Error(s) => AgentEvent::Error(s),
                                NetworkEvent::ToolApprovalRequest {
                                    call_index,
                                    name,
                                    arguments,
                                } => AgentEvent::ToolApprovalRequest {
                                    call_index,
                                    name,
                                    arguments,
                                },

                                // For ShellInputNeeded, we reconstruct a pseudo sender that POSTs to the server!
                                NetworkEvent::ShellInputNeeded { context } => {
                                    let (input_tx, mut input_rx) =
                                        mpsc::unbounded_channel::<String>();
                                    let token_clone = auth_token.clone();

                                    tokio::spawn(async move {
                                        if let Some(input) = input_rx.recv().await {
                                            let dummy_client = Client::new();
                                            let req = ShellInputReq { input };
                                            let mut request = dummy_client
                                                .post("http://127.0.0.1:8765/command/shell")
                                                .json(&req);
                                            if let Some(ref token) = token_clone {
                                                request = request.bearer_auth(token);
                                            }
                                            let _ = request.send().await;
                                        }
                                    });

                                    AgentEvent::ShellInputNeeded { context, input_tx }
                                }

                                NetworkEvent::TaskCreated(t) => AgentEvent::TaskCreated(t),
                                NetworkEvent::TaskUpdated(t) => AgentEvent::TaskUpdated(t),
                                NetworkEvent::ContextPruned { count } => {
                                    AgentEvent::ContextPruned { count }
                                }
                                NetworkEvent::ContextSummaryReady { id, summary } => {
                                    AgentEvent::ContextSummaryReady { id, summary }
                                }
                                NetworkEvent::ProviderStatus { index, connected } => {
                                    AgentEvent::ProviderStatus { index, connected }
                                }
                            };
                            let _ = tx.send(evt);
                        } else {
                            let log_path = dirs::data_local_dir()
                                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                                .join("seekr")
                                .join("errors.log");
                            if let Some(parent) = log_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&log_path)
                            {
                                use std::io::Write;
                                let _ = writeln!(f, "Failed to parse: {}", data);
                            }
                        }
                    }
                    Err(err) => {
                        let log_path = dirs::data_local_dir()
                            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                            .join("seekr")
                            .join("errors.log");
                        if let Some(parent) = log_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&log_path)
                        {
                            use std::io::Write;
                            let _ = writeln!(f, "SSE Error: {}", err);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        });

        Ok(())
    }
}
