use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyModifiers, KeyEvent};
use crate::app::{App, AppMode, Focus, SelectionMode, ChatEntry};
use crate::agent::AgentCommand;
use crate::config::AppConfig;
use crate::api::client::ApiClient;

pub async fn handle_event(app: &mut App, ev: &Event) -> Result<bool> {
    match app.mode {
        AppMode::Setup => handle_setup_event(app, ev).await,
        AppMode::Main | AppMode::AwaitingContinue => Ok(handle_main_event(app, ev).await),
        AppMode::QuitConfirm => Ok(handle_quit_confirm(app, ev)),
        AppMode::Help => {
            if let Event::Key(_) = ev {
                app.mode = AppMode::Main;
            }
            Ok(false)
        }
        AppMode::UnifiedMenu => {
            if let Event::Key(key) = ev {
                handle_unified_menu_event(app, key).await;
            }
            Ok(false)
        }
    }
}

pub fn handle_quit_confirm(app: &mut App, ev: &Event) -> bool {
    if let Event::Key(key) = ev {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => return true,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.mode = AppMode::Main,
            _ => {}
        }
    }
    false
}

pub async fn handle_setup_event(app: &mut App, ev: &Event) -> Result<bool> {
    if let Event::Key(key) = ev {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }

        match app.setup_state.current_step {
            0 => {
                if key.code == KeyCode::Enter {
                    app.setup_state.current_step = 1;
                }
            }
            1 => match key.code {
                KeyCode::Up => app.setup_state.provider_selection = app.setup_state.provider_selection.saturating_sub(1),
                KeyCode::Down => app.setup_state.provider_selection = (app.setup_state.provider_selection + 1).min(3),
                KeyCode::Enter => app.setup_state.current_step = 2,
                KeyCode::Esc => app.setup_state.current_step = 0,
                _ => {}
            },
            2 => match key.code {
                KeyCode::Enter => {
                    if !app.setup_state.api_key_input.is_empty() {
                        app.setup_state.error_message = None;
                        app.setup_state.current_step = 3;
                    } else {
                        app.setup_state.error_message = Some("API key cannot be empty".to_string());
                    }
                }
                KeyCode::Esc => app.setup_state.current_step = 1,
                KeyCode::Backspace => { app.setup_state.api_key_input.pop(); }
                KeyCode::Char(c) => {
                    app.setup_state.api_key_input.push(c);
                    app.setup_state.error_message = None;
                }
                _ => {}
            },
            3 => match key.code {
                KeyCode::Up => app.setup_state.model_selection = app.setup_state.model_selection.saturating_sub(1),
                KeyCode::Down => {
                    let models_count: usize = match app.setup_state.provider_selection {
                        0 => 2, 1 => 2, 2 => 1, _ => 5
                    };
                    app.setup_state.model_selection = (app.setup_state.model_selection + 1).min(models_count.saturating_sub(1));
                }
                KeyCode::Enter => app.setup_state.current_step = 4,
                KeyCode::Esc => app.setup_state.current_step = 2,
                _ => {}
            },
            4 => match key.code {
                KeyCode::Up => app.setup_state.auto_approve_selection = app.setup_state.auto_approve_selection.saturating_sub(1),
                KeyCode::Down => app.setup_state.auto_approve_selection = (app.setup_state.auto_approve_selection + 1).min(1),
                KeyCode::Enter => app.setup_state.current_step = 5,
                KeyCode::Esc => app.setup_state.current_step = 3,
                _ => {}
            },
            5 => match key.code {
                KeyCode::Enter => {
                    app.setup_state.current_step = 6;
                    app.setup_state.error_message = None;
                    app.setup_state.validating = true;

                    let key = app.setup_state.api_key_input.clone();
                    let model_id = match app.setup_state.provider_selection {
                        0 => if app.setup_state.model_selection == 0 { "gpt-4o" } else { "gpt-4o-mini" },
                        1 => if app.setup_state.model_selection == 0 { "deepseek-chat" } else { "deepseek-reasoner" },
                        2 => "claude-3-5-sonnet-latest",
                        _ => match app.setup_state.model_selection {
                            0 => "gpt-4o", 1 => "gpt-4o-mini", 2 => "claude-3-5-sonnet-latest", 3 => "deepseek-chat", _ => "deepseek-reasoner"
                        }
                    };
                    let base_url = AppConfig::get_default_base_url(model_id);
                    let valid = ApiClient::validate_key(&key, &base_url, model_id).await;
                    app.setup_state.validating = false;

                    match valid {
                        Ok(true) => {
                            let auto_approve = app.setup_state.auto_approve_selection == 1;
                            let working_dir = if app.setup_state.working_dir_input.is_empty() { ".".to_string() } else { app.setup_state.working_dir_input.clone() };

                            let config = AppConfig {
                                providers: vec![crate::config::ProviderConfig {
                                    name: match app.setup_state.provider_selection {
                                        0 => "OpenAI", 1 => "DeepSeek", 2 => "Anthropic", _ => "AI Provider"
                                    }.to_string(),
                                    key: app.setup_state.api_key_input.clone(),
                                    base_url: base_url.clone(),
                                    model: model_id.to_string(),
                                    timeout: None,
                                }],
                                active_provider: 0,
                                agent: crate::config::AgentConfig {
                                    max_iterations: 15,
                                    auto_approve_tools: auto_approve,
                                    working_directory: working_dir,
                                    context_window_threshold: 40,
                                    context_window_keep: 10,
                                },
                                ui: crate::config::UiConfig { theme: "dark".to_string(), show_reasoning: true },
                            };

                            if let Err(e) = config.save() {
                                app.setup_state.error_message = Some(format!("Failed to save config: {e}"));
                            } else {
                                app.manager = Some(std::sync::Arc::new(crate::manager::SeekrManager::new(config.clone())));
                                app.config = Some(config);
                                app.setup_state.current_step = 7;
                            }
                        }
                        Ok(false) => { app.setup_state.error_message = Some("Invalid API key.".to_string()); }
                        Err(e) => { app.setup_state.error_message = Some(format!("Connection error: {e}")); }
                    }
                }
                KeyCode::Esc => app.setup_state.current_step = 4,
                KeyCode::Backspace => { app.setup_state.working_dir_input.pop(); }
                KeyCode::Char(c) => { app.setup_state.working_dir_input.push(c); }
                _ => {}
            },
            6 => if key.code == KeyCode::Enter { app.setup_state.current_step = 2; app.setup_state.error_message = None; }
            7 => if key.code == KeyCode::Enter {
                app.mode = AppMode::Main;
                app.show_reasoning = true;
                app.chat_entries.push(ChatEntry::SystemInfo("Welcome to Seekr!".to_string()));
                app.start_agent();
            }
            _ => {}
        }
    }
    Ok(false)
}

pub async fn handle_main_event(app: &mut App, ev: &Event) -> bool {
    if let Event::Key(KeyEvent { code, modifiers, .. }) = ev {
        if *code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }

        if app.mode == AppMode::AwaitingContinue {
            match code {
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    app.mode = AppMode::Main;
                    app.is_streaming = true;
                    app.user_scrolled = false;
                    if let Some(ref tx) = app.agent_cmd_tx { tx.send(AgentCommand::Continue).ok(); }
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    app.mode = AppMode::Main;
                    if let Some(ref tx) = app.agent_cmd_tx { tx.send(AgentCommand::AnswerNow).ok(); }
                }
                _ => {}
            }
            return false;
        }

        if app.awaiting_approval {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => app.handle_approval(true, false),
                KeyCode::Char('n') | KeyCode::Char('N') => app.handle_approval(false, false),
                KeyCode::Char('a') | KeyCode::Char('A') => app.handle_approval(true, true),
                _ => {}
            }
            return false;
        }

        match code {
            KeyCode::F(1) => app.mode = AppMode::Help,
            KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.open_unified_menu().await;
            }
            KeyCode::Enter => {
                app.send_message();
                app.user_scrolled = false;
                app.scroll_offset = app.chat_max_scroll;
            }
            KeyCode::Esc => {
                if app.focus == Focus::Chat && app.chat_selection.mode != SelectionMode::Normal {
                    app.chat_selection.mode = SelectionMode::Normal;
                    app.chat_selection.anchor_vline = None;
                    app.chat_selection.anchor_col = None;
                } else {
                    app.mode = AppMode::QuitConfirm;
                }
            }
            KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.clear_chat();
            }
            _ => {
                match app.focus {
                    Focus::Input => handle_input_focus_keys(app, code, modifiers),
                    Focus::Chat => handle_chat_focus_keys(app, code, modifiers),
                    Focus::Tasks => handle_tasks_focus_keys(app, code, modifiers),
                }
            }
        }
    }
    false
}

fn handle_input_focus_keys(app: &mut App, code: &KeyCode, modifiers: &KeyModifiers) {
    match code {
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => app.focus = Focus::Chat,
        KeyCode::Tab => app.focus = Focus::Tasks,
        KeyCode::Char(c) => {
            app.input.insert(app.cursor_pos, *c);
            app.cursor_pos += 1;
        }
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                app.input.remove(app.cursor_pos - 1);
                app.cursor_pos -= 1;
            }
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                app.input.remove(app.cursor_pos);
            }
        }
        KeyCode::Left => app.cursor_pos = app.cursor_pos.saturating_sub(1),
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos += 1;
            }
        }
        KeyCode::Home => app.cursor_pos = 0,
        KeyCode::End => app.cursor_pos = app.input.len(),
        _ => {}
    }
}

fn handle_chat_focus_keys(app: &mut App, code: &KeyCode, _modifiers: &KeyModifiers) {
    match code {
        KeyCode::Char('i') | KeyCode::Esc => app.focus = Focus::Input,
        KeyCode::Char('j') | KeyCode::Down => {
            app.chat_selection.vline = (app.chat_selection.vline + 1).min(app.get_max_vline());
            app.ensure_vline_visible();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.chat_selection.vline = app.chat_selection.vline.saturating_sub(1);
            app.ensure_vline_visible();
        }
        KeyCode::Char('v') => {
            if app.chat_selection.mode == SelectionMode::Visual {
                app.chat_selection.mode = SelectionMode::Normal;
                app.chat_selection.anchor_vline = None;
            } else {
                app.chat_selection.mode = SelectionMode::Visual;
                app.chat_selection.anchor_vline = Some(app.chat_selection.vline);
                app.chat_selection.anchor_col = Some(app.chat_selection.col);
            }
        }
        KeyCode::Char('V') => {
            if app.chat_selection.mode == SelectionMode::VisualLine {
                app.chat_selection.mode = SelectionMode::Normal;
                app.chat_selection.anchor_vline = None;
            } else {
                app.chat_selection.mode = SelectionMode::VisualLine;
                app.chat_selection.anchor_vline = Some(app.chat_selection.vline);
            }
        }
        KeyCode::Char('y') => {
            if let Some(text) = app.get_selected_text() {
                if let Some(ref mut cb) = app.clipboard { cb.set_text(text).ok(); }
                app.chat_selection.mode = SelectionMode::Normal;
                app.chat_selection.anchor_vline = None;
            }
        }
        KeyCode::Char('c') => {
            if let Some(text) = app.copy_code_block_at_vline(app.chat_selection.vline) {
                if let Some(ref mut cb) = app.clipboard { cb.set_text(text).ok(); }
            }
        }
        _ => {}
    }
}

fn handle_tasks_focus_keys(app: &mut App, code: &KeyCode, _modifiers: &KeyModifiers) {
    match code {
        KeyCode::Tab => app.focus = Focus::Input,
        _ => {}
    }
}

pub async fn handle_unified_menu_event(app: &mut App, key: &KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = AppMode::Main,
        KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
            app.menu_state.active_tab = match app.menu_state.active_tab {
                crate::app::MenuTab::Sessions => crate::app::MenuTab::Models,
                crate::app::MenuTab::Models => crate::app::MenuTab::Providers,
                crate::app::MenuTab::Providers => crate::app::MenuTab::Settings,
                crate::app::MenuTab::Settings => crate::app::MenuTab::Help,
                crate::app::MenuTab::Help => crate::app::MenuTab::Sessions,
            };
            app.menu_state.selection_idx = 0;
            app.menu_state.scroll_offset = 0;
        }
        KeyCode::Char('h') | KeyCode::Left => {
            app.menu_state.active_tab = match app.menu_state.active_tab {
                crate::app::MenuTab::Sessions => crate::app::MenuTab::Help,
                crate::app::MenuTab::Models => crate::app::MenuTab::Sessions,
                crate::app::MenuTab::Providers => crate::app::MenuTab::Models,
                crate::app::MenuTab::Settings => crate::app::MenuTab::Providers,
                crate::app::MenuTab::Help => crate::app::MenuTab::Settings,
            };
            app.menu_state.selection_idx = 0;
            app.menu_state.scroll_offset = 0;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_state.selection_idx = app.menu_state.selection_idx.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = match app.menu_state.active_tab {
                crate::app::MenuTab::Sessions => app.sessions.len(),
                crate::app::MenuTab::Models => app.available_models.len(),
                crate::app::MenuTab::Providers => app.config.as_ref().map(|c| c.providers.len()).unwrap_or(0),
                crate::app::MenuTab::Settings => 5,
                crate::app::MenuTab::Help => 0,
            };
            if app.menu_state.selection_idx + 1 < max {
                app.menu_state.selection_idx += 1;
            }
        }
        KeyCode::Enter => {
            match app.menu_state.active_tab {
                crate::app::MenuTab::Sessions => {
                    if let Some(session) = app.sessions.get(app.menu_state.selection_idx) {
                        let id = session.id.clone();
                        app.session_id = Some(id.clone());
                        app.mode = AppMode::Main;
                        app.resume_session(id);
                        app.start_agent();
                    }
                }
                crate::app::MenuTab::Models => {
                    if let Some(model) = app.available_models.get(app.menu_state.selection_idx) {
                        let model_clone = model.clone();
                        if let Some(cfg) = app.config.as_mut() {
                            cfg.current_provider_mut().model = model_clone.clone();
                            cfg.save().ok();
                            app.mode = AppMode::Main;
                            app.chat_entries.push(ChatEntry::SystemInfo(format!("Switched to model: {}", model_clone)));
                            app.start_agent();
                        }
                    }
                }
                crate::app::MenuTab::Providers => {
                    if let Some(cfg) = app.config.as_mut() {
                        cfg.active_provider = app.menu_state.selection_idx;
                        cfg.save().ok();
                        app.mode = AppMode::Main;
                        app.chat_entries.push(ChatEntry::SystemInfo(format!("Switched to provider: {}", cfg.current_provider().name)));
                        app.start_agent();
                    }
                }
                crate::app::MenuTab::Settings => {
                    if let Some(cfg) = app.config.as_mut() {
                        match app.menu_state.selection_idx {
                            0 => {}
                            1 => {
                                cfg.agent.max_iterations = match cfg.agent.max_iterations {
                                    15 => 30, 30 => 50, 50 => 100, 100 => 200, 200 => 500, 500 => 1000, _ => 15
                                };
                            }
                            2 => { cfg.agent.auto_approve_tools = !cfg.agent.auto_approve_tools; }
                            4 => {
                                cfg.ui.show_reasoning = !cfg.ui.show_reasoning;
                                app.show_reasoning = cfg.ui.show_reasoning;
                            }
                            _ => {}
                        }
                        cfg.save().ok();
                    }
                }
                _ => {}
            }
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            if app.menu_state.active_tab == crate::app::MenuTab::Sessions {
                app.delete_session_at(app.menu_state.selection_idx).await;
            }
        }
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.fetch_available_models();
        }
        _ => {}
    }
}
