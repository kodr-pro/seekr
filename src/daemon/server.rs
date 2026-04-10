use axum::{
    Json, Router,
    extract::State,
    response::sse::{Event, Sse},
    routing::{get, post},
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::CorsLayer;

use crate::agent::{AgentCommand, AgentEvent, loop_mod::AgentLoop};
use crate::config::AppConfig;
use crate::manager::SeekrManager;

#[derive(Clone)]
pub struct DaemonState {
    pub config: AppConfig,
    pub manager: Arc<SeekrManager>,
    pub cmd_tx: Arc<Mutex<Option<mpsc::UnboundedSender<AgentCommand>>>>,
    pub shell_input_tx: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
    pub evt_broadcast: broadcast::Sender<AgentEvent>,
}

#[derive(Deserialize, Serialize)]
pub struct ChatMessageReq {
    pub message: String,
}

#[derive(Deserialize, Serialize)]
pub struct ToolApprovalReq {
    pub approved: bool,
    pub always: bool,
}

#[derive(Deserialize, Serialize)]
pub struct StartAgentReq {
    pub session_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct ShellInputReq {
    pub input: String,
}

pub async fn start_server() -> anyhow::Result<()> {
    let config = AppConfig::load().unwrap_or_else(|_| AppConfig::default());
    let manager = std::sync::Arc::new(SeekrManager::new(config.clone()));

    // Broadcast channel for distributing AgentEvents to all connected clients (SSE)
    let (evt_broadcast, _) = broadcast::channel(1000);

    let state = DaemonState {
        config,
        manager,
        cmd_tx: Arc::new(Mutex::new(None)),
        shell_input_tx: Arc::new(Mutex::new(None)),
        evt_broadcast,
    };

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/events", get(sse_handler))
        .route("/start", post(start_handler))
        .route("/chat", post(chat_handler))
        .route("/command/approve", post(approve_handler))
        .route("/command/shutdown", post(shutdown_handler))
        .route("/command/check_connection", post(check_connection_handler))
        .route("/command/shell", post(shell_input_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8765));
    println!("Seekr daemon listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", content = "data")]
pub enum NetworkEvent {
    ContentDelta(String),
    ReasoningDelta(String),
    ToolCallStart {
        name: String,
        arguments: String,
    },
    ToolCallResult {
        name: String,
        result: String,
    },
    Activity(crate::tools::ActivityEntry),
    TokenUsage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    },
    IterationUpdate(u32),
    TurnComplete,
    MaxIterationsReached,
    Error(String),
    ToolApprovalRequest {
        call_index: usize,
        name: String,
        arguments: String,
    },
    ShellInputNeeded {
        context: String,
    },
    TaskCreated(crate::tools::task::Task),
    TaskUpdated(crate::tools::task::Task),
    ContextPruned {
        count: usize,
    },
    ContextSummaryReady {
        id: String,
        summary: String,
    },
    ProviderStatus {
        index: usize,
        connected: bool,
    },
}

async fn sse_handler(
    State(state): State<DaemonState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.evt_broadcast.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|res| {
        match res {
            Ok(evt) => {
                let net_evt = match evt {
                    AgentEvent::ContentDelta(s) => NetworkEvent::ContentDelta(s),
                    AgentEvent::ReasoningDelta(s) => NetworkEvent::ReasoningDelta(s),
                    AgentEvent::ToolCallStart { name, arguments } => {
                        NetworkEvent::ToolCallStart { name, arguments }
                    }
                    AgentEvent::ToolCallResult { name, result } => {
                        NetworkEvent::ToolCallResult { name, result }
                    }
                    AgentEvent::Activity(a) => NetworkEvent::Activity(a),
                    AgentEvent::TokenUsage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                    } => NetworkEvent::TokenUsage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                    },
                    AgentEvent::IterationUpdate(n) => NetworkEvent::IterationUpdate(n),
                    AgentEvent::TurnComplete => NetworkEvent::TurnComplete,
                    AgentEvent::MaxIterationsReached => NetworkEvent::MaxIterationsReached,
                    AgentEvent::Error(e) => NetworkEvent::Error(e.to_string()),
                    AgentEvent::ToolApprovalRequest {
                        call_index,
                        name,
                        arguments,
                    } => NetworkEvent::ToolApprovalRequest {
                        call_index,
                        name,
                        arguments,
                    },
                    AgentEvent::ShellInputNeeded { context, .. } => {
                        NetworkEvent::ShellInputNeeded { context }
                    }
                    AgentEvent::TaskCreated(t) => NetworkEvent::TaskCreated(t),
                    AgentEvent::TaskUpdated(t) => NetworkEvent::TaskUpdated(t),
                    AgentEvent::ContextPruned { count } => NetworkEvent::ContextPruned { count },
                    AgentEvent::ContextSummaryReady { id, summary } => {
                        NetworkEvent::ContextSummaryReady { id, summary }
                    }
                    AgentEvent::ProviderStatus { index, connected } => {
                        NetworkEvent::ProviderStatus { index, connected }
                    }
                };

                if let Ok(json) = serde_json::to_string(&net_evt) {
                    Some(Ok(Event::default().data(json)))
                } else {
                    None
                }
            }
            Err(_) => None, // RecvError::Lagged
        }
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}

async fn start_handler(
    State(state): State<DaemonState>,
    Json(payload): Json<StartAgentReq>,
) -> &'static str {
    let mut cmd_tx_guard = state.cmd_tx.lock().await;

    // Shutdown existing agent if any
    if let Some(tx) = cmd_tx_guard.as_ref() {
        let _ = tx.send(AgentCommand::Shutdown);
    }

    let (evt_tx, mut evt_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();

    let broadcast = state.evt_broadcast.clone();
    let shell_input_tx_state = state.shell_input_tx.clone();

    // Spawn a forwarder task
    tokio::spawn(async move {
        while let Some(evt) = evt_rx.recv().await {
            if let AgentEvent::ShellInputNeeded { ref input_tx, .. } = evt {
                let mut guard = shell_input_tx_state.lock().await;
                *guard = Some(input_tx.clone());
            }
            let _ = broadcast.send(evt);
        }
    });

    let config = state.config.clone();
    let registry = state.manager.tool_registry();

    let mcp_manager = state.manager.mcp_manager();

    let agent = if let Some(sid) = payload.session_id {
        AgentLoop::resume(config, &sid, evt_tx, cmd_rx, cmd_tx.clone(), registry, crate::agent::system_prompt::AgentRole::Main, mcp_manager)
    } else {
        Ok(AgentLoop::new(
            config,
            evt_tx,
            cmd_rx,
            cmd_tx.clone(),
            registry,
            crate::agent::system_prompt::AgentRole::Main,
            mcp_manager,
        ))
    };

    match agent {
        Ok(agent) => {
            tokio::spawn(agent.run());
            *cmd_tx_guard = Some(cmd_tx);
            "Started"
        }
        Err(e) => {
            eprintln!("Failed to start agent: {}", e);
            "Error"
        }
    }
}

async fn chat_handler(
    State(state): State<DaemonState>,
    Json(payload): Json<ChatMessageReq>,
) -> &'static str {
    let tx_guard = state.cmd_tx.lock().await;
    if let Some(tx) = tx_guard.as_ref() {
        let _ = tx.send(AgentCommand::UserMessage(payload.message));
        "Sent"
    } else {
        "Agent not started"
    }
}

async fn approve_handler(
    State(state): State<DaemonState>,
    Json(payload): Json<ToolApprovalReq>,
) -> &'static str {
    let tx_guard = state.cmd_tx.lock().await;
    if let Some(tx) = tx_guard.as_ref() {
        if payload.always {
            let _ = tx.send(AgentCommand::ToolAlwaysApprove);
        } else if payload.approved {
            let _ = tx.send(AgentCommand::ToolApproved { call_index: 0 });
        } else {
            let _ = tx.send(AgentCommand::ToolDenied { call_index: 0 });
        }
        "Sent"
    } else {
        "Agent not started"
    }
}

async fn shutdown_handler(State(state): State<DaemonState>) -> &'static str {
    let tx_guard = state.cmd_tx.lock().await;
    if let Some(tx) = tx_guard.as_ref() {
        let _ = tx.send(AgentCommand::Shutdown);
        "Sent"
    } else {
        "Agent not started"
    }
}

async fn check_connection_handler(State(state): State<DaemonState>) -> &'static str {
    let tx_guard = state.cmd_tx.lock().await;
    if let Some(tx) = tx_guard.as_ref() {
        let _ = tx.send(AgentCommand::CheckConnection);
        "Sent"
    } else {
        "Agent not started"
    }
}

async fn shell_input_handler(
    State(state): State<DaemonState>,
    Json(payload): Json<ShellInputReq>,
) -> &'static str {
    let mut guard = state.shell_input_tx.lock().await;
    if let Some(tx) = guard.take() {
        let _ = tx.send(payload.input);
        "Sent"
    } else {
        "No process waiting for input"
    }
}
