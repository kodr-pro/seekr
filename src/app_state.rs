use crate::agent::{AgentCommand, AgentEvent};
use crate::tools::ActivityEntry;
use crate::ui::layout::AppLayout;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct UiState {
    pub scroll_offset: u16,
    pub chat_max_scroll: u16,
    pub user_scrolled: bool,
    pub show_reasoning: bool,
    pub last_chat_width: u16,
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub needs_recompute_vlines: bool,
    pub layout: Option<AppLayout>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            chat_max_scroll: 0,
            user_scrolled: false,
            show_reasoning: true,
            last_chat_width: 80,
            terminal_width: 0,
            terminal_height: 0,
            needs_recompute_vlines: true,
            layout: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct AgentState {
    pub cmd_tx: Option<mpsc::UnboundedSender<AgentCommand>>,
    pub event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    pub total_tokens: u32,
    pub iteration: u32,
    pub connected: bool,
    pub awaiting_approval: bool,
    pub streaming_content: String,
    pub streaming_reasoning: String,
    pub is_streaming: bool,
    pub live_activities: Vec<ActivityEntry>,
    pub activities: Vec<ActivityEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub session_id: Option<String>,
    pub sessions: Vec<crate::session::SessionMetadata>,
    pub session_list_error: Option<String>,
    pub available_models: Vec<String>,
}
