// api/stream.rs - SSE (Server-Sent Events) streaming parser
//
// Parses the chunked SSE response from DeepSeek's streaming API.
// Handles partial tool calls by accumulating deltas, and separates
// reasoning_content from regular content for the reasoner model.

use anyhow::Result;
use futures::StreamExt;
use reqwest::Response;

use super::types::{StreamChunk, StreamDelta, StreamToolCall, ToolCall, FunctionCall};

/// Events emitted by the SSE stream parser to the caller
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text content from the assistant
    ContentDelta(String),
    /// A chunk of reasoning content (deepseek-reasoner)
    ReasoningDelta(String),
    /// A fully-assembled tool call, ready for execution
    ToolCallComplete(ToolCall),
    /// Token usage information (sent at the end)
    Usage { prompt_tokens: u32, completion_tokens: u32, total_tokens: u32 },
    /// The stream has finished
    Done,
    /// An error occurred during streaming
    Error(String),
}

/// Accumulated state for tool calls being built from streaming deltas
#[derive(Debug, Default)]
struct ToolCallAccumulator {
    /// In-progress tool calls keyed by index
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
    /// Apply a streaming tool call delta, extending or creating an entry
    fn apply_delta(&mut self, delta: &StreamToolCall) {
        let idx = delta.index as usize;
        // Ensure we have enough slots
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
    }

    /// Finalize all accumulated tool calls into complete ToolCall structs
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
    }
}

/// Parse an SSE stream from the DeepSeek API and yield StreamEvents.
///
/// This reads the raw byte stream, splits on SSE `data:` lines, parses
/// each JSON chunk, and accumulates partial tool calls until the stream ends.
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

        // Append the new bytes to our line buffer
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            // SSE format: "data: <json>" or "data: [DONE]"
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();

                if data == "[DONE]" {
                    // Finalize any accumulated tool calls
                    let tool_calls = tool_acc.finalize();
                    for tc in tool_calls {
                        let _ = event_tx.send(StreamEvent::ToolCallComplete(tc));
                    }
                    let _ = event_tx.send(StreamEvent::Done);
                    return Ok(());
                }

                // Parse the JSON chunk
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

    // Stream ended without [DONE] - finalize what we have
    let tool_calls = tool_acc.finalize();
    for tc in tool_calls {
        let _ = event_tx.send(StreamEvent::ToolCallComplete(tc));
    }
    let _ = event_tx.send(StreamEvent::Done);
    Ok(())
}

/// Process a single parsed streaming chunk
fn process_chunk(
    chunk: &StreamChunk,
    tool_acc: &mut ToolCallAccumulator,
    event_tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
) {
    // Handle usage info (typically in the final chunk)
    if let Some(ref usage) = chunk.usage {
        let _ = event_tx.send(StreamEvent::Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        });
    }

    for choice in &chunk.choices {
        let delta: &StreamDelta = &choice.delta;

        // Regular text content
        if let Some(ref content) = delta.content {
            if !content.is_empty() {
                let _ = event_tx.send(StreamEvent::ContentDelta(content.clone()));
            }
        }

        // Reasoning content (deepseek-reasoner model)
        if let Some(ref reasoning) = delta.reasoning_content {
            if !reasoning.is_empty() {
                let _ = event_tx.send(StreamEvent::ReasoningDelta(reasoning.clone()));
            }
        }

        // Tool call deltas - accumulate them
        if let Some(ref tool_calls) = delta.tool_calls {
            for tc_delta in tool_calls {
                tool_acc.apply_delta(tc_delta);
            }
        }
    }
}
