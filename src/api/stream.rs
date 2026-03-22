use anyhow::Result;
use futures::StreamExt;
use reqwest::Response;

use super::types::{FunctionCall, StreamChunk, StreamToolCall, ToolCall};

#[derive(Debug, Clone)]
pub enum StreamEvent {
    ContentDelta(String),
    ReasoningDelta(String),
    ToolCallComplete(ToolCall),
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    },
    Done,
    Error(String),
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    calls: Vec<PartialToolCall>,
}

#[derive(Debug, Default, Clone)]
struct PartialToolCall {
    id: Option<String>,
    call_type: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallAccumulator {
    fn apply_delta(&mut self, delta: &StreamToolCall) {
        let idx = delta.index as usize;
        while self.calls.len() <= idx {
            self.calls.push(PartialToolCall::default());
        }

        let entry = &mut self.calls[idx];
        if let Some(ref id) = delta.id {
            entry.id = Some(id.clone());
        }
        if let Some(ref ct) = delta.call_type {
            entry.call_type = Some(ct.clone());
        }
        if let Some(ref func) = delta.function {
            if let Some(ref name) = func.name {
                entry.name = Some(name.clone());
            }
            if let Some(ref args) = func.arguments {
                entry.arguments.push_str(args);
            }
        }
    } // apply_delta

    fn finalize(self) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .filter_map(|partial| {
                let id = partial.id?;
                let name = partial.name?;
                Some(ToolCall {
                    id,
                    call_type: partial.call_type.unwrap_or_else(|| "function".to_string()),
                    function: FunctionCall {
                        name,
                        arguments: partial.arguments,
                    },
                })
            })
            .collect()
    } // finalize
} // impl ToolCallAccumulator

pub async fn parse_sse_stream(
    response: Response,
    event_tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
) -> Result<()> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut tool_acc = ToolCallAccumulator::default();

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = match chunk_result {
            Ok(bytes) => bytes,
            Err(e) => {
                let _ = event_tx.send(StreamEvent::Error(format!("Stream error: {e}")));
                break;
            }
        };

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();

                if data == "[DONE]" {
                    let tool_calls = tool_acc.finalize();
                    for tc in tool_calls {
                        let _ = event_tx.send(StreamEvent::ToolCallComplete(tc));
                    }
                    let _ = event_tx.send(StreamEvent::Done);
                    return Ok(());
                }

                match serde_json::from_str::<StreamChunk>(data) {
                    Ok(chunk) => {
                        process_chunk(&chunk, &mut tool_acc, &event_tx);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse SSE chunk: {e}, data: {data}");
                    }
                }
            }
        }
    }

    let tool_calls = tool_acc.finalize();
    for tc in tool_calls {
        let _ = event_tx.send(StreamEvent::ToolCallComplete(tc));
    }
    let _ = event_tx.send(StreamEvent::Done);
    Ok(())
} // parse_sse_stream

fn process_chunk(
    chunk: &StreamChunk,
    tool_acc: &mut ToolCallAccumulator,
    event_tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
) {
    if let Some(ref usage) = chunk.usage {
        let _ = event_tx.send(StreamEvent::Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        });
    }

    for choice in &chunk.choices {
        let delta = &choice.delta;

        if let Some(ref content) = delta.content
            && !content.is_empty()
        {
            let _ = event_tx.send(StreamEvent::ContentDelta(content.clone()));
        }

        if let Some(ref reasoning) = delta.reasoning_content
            && !reasoning.is_empty()
        {
            let _ = event_tx.send(StreamEvent::ReasoningDelta(reasoning.clone()));
        }

        if let Some(ref tool_calls) = delta.tool_calls {
            for tc_delta in tool_calls {
                tool_acc.apply_delta(tc_delta);
            }
        }
    }
} // process_chunk

pub async fn parse_anthropic_sse_stream(
    response: Response,
    event_tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
) -> Result<()> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();

    let mut prompt_tokens = 0;

    let mut current_tool_id: Option<String> = None;
    let mut current_tool_name: Option<String> = None;
    let mut current_tool_args = String::new();

    let mut event_type = String::new();

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = match chunk_result {
            Ok(bytes) => bytes,
            Err(e) => {
                let _ = event_tx.send(StreamEvent::Error(format!("Stream error: {e}")));
                break;
            }
        };

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            if let Some(event) = line.strip_prefix("event:") {
                event_type = event.trim().to_string();
                continue;
            }

            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();

                if data == "[DONE]" {
                    continue;
                }

                let value_res: Result<serde_json::Value, _> = serde_json::from_str(data);
                if let Ok(value) = value_res {
                    match event_type.as_str() {
                        "message_start" => {
                            if let Some(input_tokens) = value
                                .pointer("/message/usage/input_tokens")
                                .and_then(|v| v.as_u64())
                            {
                                prompt_tokens = input_tokens as u32;
                            }
                        }
                        "content_block_start" => {
                            if let Some(cb_type) = value
                                .pointer("/content_block/type")
                                .and_then(|v| v.as_str())
                                && cb_type == "tool_use"
                            {
                                current_tool_id = value
                                    .pointer("/content_block/id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                current_tool_name = value
                                    .pointer("/content_block/name")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                current_tool_args.clear();
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta_type) =
                                value.pointer("/delta/type").and_then(|v| v.as_str())
                            {
                                if delta_type == "text_delta" {
                                    if let Some(text) =
                                        value.pointer("/delta/text").and_then(|v| v.as_str())
                                    {
                                        let _ = event_tx
                                            .send(StreamEvent::ContentDelta(text.to_string()));
                                    }
                                } else if delta_type == "input_json_delta"
                                    && let Some(text) = value
                                        .pointer("/delta/partial_json")
                                        .and_then(|v| v.as_str())
                                {
                                    current_tool_args.push_str(text);
                                }
                            }
                        }
                        "content_block_stop" => {
                            if let (Some(id), Some(name)) =
                                (current_tool_id.take(), current_tool_name.take())
                            {
                                let tc = ToolCall {
                                    id,
                                    call_type: "function".to_string(),
                                    function: FunctionCall {
                                        name,
                                        arguments: current_tool_args.clone(),
                                    },
                                };
                                let _ = event_tx.send(StreamEvent::ToolCallComplete(tc));
                                current_tool_args.clear();
                            }
                        }
                        "message_delta" => {
                            if let Some(output_tokens) = value
                                .pointer("/usage/output_tokens")
                                .and_then(|v| v.as_u64())
                            {
                                let completion_tokens = output_tokens as u32;
                                let _ = event_tx.send(StreamEvent::Usage {
                                    prompt_tokens,
                                    completion_tokens,
                                    total_tokens: prompt_tokens + completion_tokens,
                                });
                            }
                        }
                        "message_stop" => {
                            let _ = event_tx.send(StreamEvent::Done);
                            return Ok(());
                        }
                        "error" => {
                            let _ = event_tx
                                .send(StreamEvent::Error(format!("Anthropic error: {}", data)));
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let _ = event_tx.send(StreamEvent::Done);
    Ok(())
}
