use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{Metadata, ScriptTool, Tool};
use anyhow::{Result, anyhow};
use std::sync::Arc;

pub fn parse_markdown_skill(content: &str, working_dir: &str) -> Result<(Metadata, Vec<Arc<dyn Tool>>)> {
    let mut lines = content.lines().peekable();
    
    // Parse Skill Header
    let name_line = lines.next().ok_or_else(|| anyhow!("Empty file"))?;
    if !name_line.starts_with("# Skill:") {
        return Err(anyhow!("Missing '# Skill: [Name]' header"));
    }
    let name = name_line.trim_start_matches("# Skill:").trim().to_string();
    
    let mut description = String::new();
    while let Some(line) = lines.peek() {
        if line.starts_with("---") {
            break;
        }
        description.push_str(line);
        description.push('\n');
        lines.next();
    }
    let description = description.trim().to_string();

    // Parse Frontmatter (Version)
    let mut version = "1.0.0".to_string();
    if let Some(line) = lines.next() {
        if line.starts_with("---") {
            while let Some(line) = lines.next() {
                if line.starts_with("---") {
                    break;
                }
                if line.starts_with("version:") {
                    version = line.trim_start_matches("version:").trim().to_string();
                }
            }
        }
    }

    let metadata = Metadata { name, description, version };
    let mut tools = Vec::new();

    // Parse Tools
    while let Some(line) = lines.next() {
        if line.starts_with("## Tool:") {
            let tool_name = line.trim_start_matches("## Tool:").trim().to_string();
            let mut tool_description = String::new();
            
            while let Some(line) = lines.peek() {
                if line.starts_with("###") || line.starts_with("## Tool:") {
                    break;
                }
                tool_description.push_str(line);
                tool_description.push('\n');
                lines.next();
            }
            
            let mut parameters = serde_json::json!({"type": "object", "properties": {}});
            let mut command = String::new();

            while let Some(line) = lines.next() {
                if line.starts_with("## Tool:") {
                    // Start of next tool, but we are in a loop so we need to back up? 
                    // Actually we used next() here. Let's use a cleaner loop.
                    break; 
                }
                
                if line.starts_with("### Parameters") {
                    // Expecting ```json block
                    while let Some(l) = lines.next() {
                        if l.starts_with("```") {
                            let mut json_str = String::new();
                            while let Some(jl) = lines.next() {
                                if jl.starts_with("```") {
                                    break;
                                }
                                json_str.push_str(jl);
                                json_str.push('\n');
                            }
                            parameters = serde_json::from_str(&json_str).unwrap_or(parameters);
                            break;
                        }
                    }
                } else if line.starts_with("### Command") {
                    // Expecting ```bash block (or similar)
                    while let Some(l) = lines.next() {
                        if l.starts_with("```") {
                            while let Some(cl) = lines.next() {
                                if cl.starts_with("```") {
                                    break;
                                }
                                command.push_str(cl);
                                command.push('\n');
                            }
                            break;
                        }
                    }
                }

                if lines.peek().map(|l| l.starts_with("## Tool:")).unwrap_or(false) {
                    break;
                }
            }

            let tool_def = ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: tool_name.clone(),
                    description: tool_description.trim().to_string(),
                    parameters,
                },
            };

            tools.push(Arc::new(ScriptTool {
                name: tool_name,
                definition: tool_def,
                command: command.trim().to_string(),
                working_dir: working_dir.to_string(),
            }) as Arc<dyn Tool>);
        }
    }

    Ok((metadata, tools))
}
