use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::sse::{Event, Sse},
    routing::{get, post},
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{Any, CorsLayer};

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

pub fn pid_file_path() -> Option<std::path::PathBuf> {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .map(|d| d.join("seekr").join("daemon.pid"))
}

pub fn read_pid_file() -> Option<u32> {
    let path = pid_file_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    content.trim().parse().ok()
}

pub fn write_pid_file() -> std::io::Result<()> {
    let path = pid_file_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Cannot determine runtime directory")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, std::process::id().to_string())
}

pub fn remove_pid_file() {
    if let Some(path) = pid_file_path() {
        let _ = std::fs::remove_file(path);
    }
}

pub fn token_file_path() -> Option<std::path::PathBuf> {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .map(|d| d.join("seekr").join("daemon.token"))
}

pub fn generate_auth_token() -> String {
    use std::fmt::Write;
    let mut token = [0u8; 32];
    getrandom::fill(&mut token).unwrap_or_else(|_| {
        for (i, byte) in token.iter_mut().enumerate() {
            *byte = ((i as u64).wrapping_mul(std::process::id() as u64)
                ^ std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64) as u8;
        }
    });
    let mut hex = String::with_capacity(64);
    for byte in &token {
        write!(hex, "{:02x}", byte).unwrap();
    }
    hex
}

pub fn write_token_file(token: &str) -> std::io::Result<()> {
    let path = token_file_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Cannot determine runtime directory")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, token)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&path)?.permissions();
        if perms.mode() != 0o600 {
            let mut perms = perms;
            perms.set_mode(0o600);
            std::fs::set_permissions(&path, perms)?;
        }
    }

    Ok(())
}

pub fn read_token_file() -> Option<String> {
    let path = token_file_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    let token = content.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

pub fn remove_token_file() {
    if let Some(path) = token_file_path() {
        let _ = std::fs::remove_file(path);
    }
}

async fn auth_middleware(
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    if req.uri().path() == "/health" {
        return Ok(next.run(req).await);
    }

    let expected = match read_token_file() {
        Some(t) => t,
        None => return Ok(next.run(req).await),
    };

    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if auth_header == format!("Bearer {}", expected) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

pub async fn start_server() -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 8765));

    if tokio::net::TcpListener::bind(addr).await.is_err() {
        let pid = read_pid_file();
        match pid {
            Some(pid) if is_process_running(pid) => {
                anyhow::bail!(
                    "Port 8765 is already in use by seekr daemon (PID {}). \
                     Run `seekr daemon stop` to shut it down.",
                    pid
                );
            }
            _ => {
                eprintln!(
                    "Port 8765 is in use (possibly a stale seekr daemon). \
                     Removing stale PID file and retrying..."
                );
                remove_pid_file();
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|_| anyhow::anyhow!("Port 8765 is still in use after retry."))?;
        start_server_with_listener(listener).await
    } else {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        start_server_with_listener(listener).await
    }
}

async fn start_server_with_listener(
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    write_pid_file()?;

    let auth_token = generate_auth_token();
    write_token_file(&auth_token)?;

    let config = AppConfig::load().unwrap_or_else(|_| AppConfig::default());
    let manager = std::sync::Arc::new(SeekrManager::new(config.clone()));

    let (evt_broadcast, _) = broadcast::channel(1000);

    let state = DaemonState {
        config,
        manager,
        cmd_tx: Arc::new(Mutex::new(None)),
        shell_input_tx: Arc::new(Mutex::new(None)),
        evt_broadcast,
    };

    let localhost_origin = "http://127.0.0.1:8765"
        .parse::<axum::http::HeaderValue>()
        .expect("hardcoded localhost origin is valid");
    let cors = CorsLayer::new()
        .allow_origin(localhost_origin)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/events", get(sse_handler))
        .route("/start", post(start_handler))
        .route("/chat", post(chat_handler))
        .route("/command/approve", post(approve_handler))
        .route("/command/shutdown", post(shutdown_handler))
        .route("/command/check_connection", post(check_connection_handler))
        .route("/command/shell", post(shell_input_handler))
        .route("/command/continue", post(continue_handler))
        .route("/command/answer_now", post(answer_now_handler))
        .layer(cors)
        .layer(middleware::from_fn(auth_middleware))
        .with_state(state);

    println!("Seekr daemon listening on {}", listener.local_addr()?);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    remove_pid_file();
    remove_token_file();
    Ok(())
}

async fn health_handler() -> &'static str {
    "OK"
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.ok();
    remove_pid_file();
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
        AgentLoop::resume(
            config,
            &sid,
            evt_tx,
            cmd_rx,
            cmd_tx.clone(),
            registry,
            crate::agent::system_prompt::AgentRole::Main,
            mcp_manager,
        )
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

async fn continue_handler(State(state): State<DaemonState>) -> &'static str {
    let tx_guard = state.cmd_tx.lock().await;
    if let Some(tx) = tx_guard.as_ref() {
        let _ = tx.send(AgentCommand::Continue);
        "Sent"
    } else {
        "Agent not started"
    }
}

async fn answer_now_handler(State(state): State<DaemonState>) -> &'static str {
    let tx_guard = state.cmd_tx.lock().await;
    if let Some(tx) = tx_guard.as_ref() {
        let _ = tx.send(AgentCommand::AnswerNow);
        "Sent"
    } else {
        "Agent not started"
    }
}
