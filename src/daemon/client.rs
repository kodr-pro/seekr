use crate::agent::AgentEvent;
use reqwest::Client;
use tokio::sync::mpsc;

use super::server::{NetworkEvent, ChatMessageReq, StartAgentReq, ToolApprovalReq, ShellInputReq};

pub struct DaemonClient {
    http: Client,
    base_url: String,
}

impl DaemonClient {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
            base_url: "http://127.0.0.1:8765".to_string(),
        }
    }

    pub async fn check_health(&self) -> bool {
        if let Ok(res) = self.http.get(&format!("{}/health", self.base_url)).send().await {
            res.status().is_success()
        } else {
            false
        }
    }

    pub async fn start_agent(&self, session_id: Option<String>) -> anyhow::Result<()> {
        let req = StartAgentReq { session_id };
        self.http.post(&format!("{}/start", self.base_url))
            .json(&req)
            .send()
            .await?;
        Ok(())
    }

    pub async fn send_chat(&self, message: String) -> anyhow::Result<()> {
        let req = ChatMessageReq { message };
        self.http.post(&format!("{}/chat", self.base_url))
            .json(&req)
            .send()
            .await?;
        Ok(())
    }

    pub async fn send_approval(&self, approved: bool, always: bool) -> anyhow::Result<()> {
        let req = ToolApprovalReq { approved, always };
        self.http.post(&format!("{}/command/approve", self.base_url))
            .json(&req)
            .send()
            .await?;
        Ok(())
    }

    pub async fn send_shutdown(&self) -> anyhow::Result<()> {
        self.http.post(&format!("{}/command/shutdown", self.base_url))
            .send()
            .await?;
        Ok(())
    }

    pub async fn send_check_connection(&self) -> anyhow::Result<()> {
        self.http.post(&format!("{}/command/check_connection", self.base_url))
            .send()
            .await?;
        Ok(())
    }

    pub async fn send_shell_input(&self, input: String) -> anyhow::Result<()> {
        let req = ShellInputReq { input };
        self.http.post(&format!("{}/command/shell", self.base_url))
            .json(&req)
            .send()
            .await?;
        Ok(())
    }

    pub async fn subscribe_events(&self, tx: mpsc::UnboundedSender<AgentEvent>) -> anyhow::Result<()> {
        use reqwest_eventsource::{EventSource, Event};
        use futures_util::StreamExt;
        
        let url = format!("{}/events", self.base_url);
        let check_url = format!("{}/command/check_connection", self.base_url);
        let mut es = EventSource::get(url);
        
        tokio::spawn(async move {
            while let Some(event) = es.next().await {
                match event {
                    Ok(Event::Open) => {
                        // Crucial: Ask the daemon to probe the AI connection now that our SSE channel is guaranteed open
                        let dummy = Client::new();
                        let _ = dummy.post(&check_url).send().await;
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
                                NetworkEvent::ToolCallStart { name, arguments } => AgentEvent::ToolCallStart { name, arguments },
                                NetworkEvent::ToolCallResult { name, result } => AgentEvent::ToolCallResult { name, result },
                                NetworkEvent::Activity(a) => AgentEvent::Activity(a),
                                NetworkEvent::TokenUsage { prompt_tokens, completion_tokens, total_tokens } => AgentEvent::TokenUsage { prompt_tokens, completion_tokens, total_tokens },
                                NetworkEvent::IterationUpdate(n) => AgentEvent::IterationUpdate(n),
                                NetworkEvent::TurnComplete => AgentEvent::TurnComplete,
                                NetworkEvent::MaxIterationsReached => AgentEvent::MaxIterationsReached,
                                NetworkEvent::Error(s) => AgentEvent::Error(s),
                                NetworkEvent::ToolApprovalRequest { call_index, name, arguments } => AgentEvent::ToolApprovalRequest { call_index, name, arguments },
                                
                                // For ShellInputNeeded, we reconstruct a pseudo sender that POSTs to the server!
                                NetworkEvent::ShellInputNeeded { context } => {
                                    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<String>();
                                    
                                    tokio::spawn(async move {
                                        if let Some(input) = input_rx.recv().await {
                                            let dummy_client = Client::new();
                                            let req = ShellInputReq { input };
                                            let _ = dummy_client.post("http://127.0.0.1:8765/command/shell")
                                                .json(&req)
                                                .send()
                                                .await;
                                        }
                                    });

                                    AgentEvent::ShellInputNeeded { context, input_tx }
                                },
                                
                                NetworkEvent::TaskCreated(t) => AgentEvent::TaskCreated(t),
                                NetworkEvent::TaskUpdated(t) => AgentEvent::TaskUpdated(t),
                                NetworkEvent::ContextPruned { count } => AgentEvent::ContextPruned { count },
                                NetworkEvent::ContextSummaryReady { id, summary } => AgentEvent::ContextSummaryReady { id, summary },
                                NetworkEvent::ProviderStatus { index, connected } => AgentEvent::ProviderStatus { index, connected },
                            };
                            let _ = tx.send(evt);
                        } else {
                            let mut f = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/err.log").unwrap();
                            use std::io::Write;
                            writeln!(f, "Failed to parse: {}", data).unwrap();
                        }
                    }
                    Err(err) => {
                        let mut f = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/err.log").unwrap();
                        use std::io::Write;
                        writeln!(f, "SSE Error: {}", err).unwrap();
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        });

        Ok(())
    }
}
