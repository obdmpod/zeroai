use crate::config::Config;
use crate::memory::{self, Memory, MemoryCategory};
use crate::observability::{self, Observer, ObserverEvent};
use crate::providers::{self, Provider};
use crate::runtime;
use crate::security::SecurityPolicy;
use crate::tools::{self, Tool};
use crate::util::truncate_with_ellipsis;
use anyhow::Result;
use serde_json::Value;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Instant;

/// Maximum tool-calling iterations per user message to prevent runaway loops.
const MAX_TOOL_ITERATIONS: usize = 10;

/// A parsed tool invocation from the LLM response.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Parse all `<tool_call>...</tool_call>` blocks from a response string.
pub fn parse_tool_calls(response: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut search_from = 0;

    loop {
        let Some(start_tag) = response[search_from..].find("<tool_call>") else {
            break;
        };
        let content_start = search_from + start_tag + "<tool_call>".len();

        let Some(end_tag) = response[content_start..].find("</tool_call>") else {
            break;
        };
        let content_end = content_start + end_tag;

        let json_str = response[content_start..content_end].trim();
        if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
            if let (Some(name), arguments) = (
                parsed.get("name").and_then(|n| n.as_str()),
                parsed.get("arguments").cloned().unwrap_or(Value::Object(Default::default())),
            ) {
                calls.push(ToolCall {
                    name: name.to_string(),
                    arguments,
                });
            }
        }

        search_from = content_end + "</tool_call>".len();
    }

    calls
}

/// Extract the text portions of a response (everything outside `<tool_call>` blocks).
fn extract_text_outside_tool_calls(response: &str) -> String {
    let mut text = String::new();
    let mut search_from = 0;

    loop {
        let Some(start_tag) = response[search_from..].find("<tool_call>") else {
            text.push_str(&response[search_from..]);
            break;
        };
        text.push_str(&response[search_from..search_from + start_tag]);

        let content_start = search_from + start_tag + "<tool_call>".len();
        let Some(end_tag) = response[content_start..].find("</tool_call>") else {
            break;
        };
        search_from = content_start + end_tag + "</tool_call>".len();
    }

    let trimmed = text.trim();
    trimmed.to_string()
}

/// Execute parsed tool calls against the tool registry.
async fn execute_tool_calls(
    tools: &[Box<dyn Tool>],
    calls: &[ToolCall],
) -> Vec<(String, crate::tools::ToolResult)> {
    let mut results = Vec::with_capacity(calls.len());

    for call in calls {
        let tool = tools.iter().find(|t| t.name() == call.name);
        let result = match tool {
            Some(t) => match t.execute(call.arguments.clone()).await {
                Ok(r) => r,
                Err(e) => crate::tools::ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Tool execution error: {e}")),
                },
            },
            None => crate::tools::ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Unknown tool: {}", call.name)),
            },
        };
        results.push((call.name.clone(), result));
    }

    results
}

/// Format tool results as XML blocks for feeding back to the LLM.
pub fn format_tool_results(results: &[(String, crate::tools::ToolResult)]) -> String {
    let mut out = String::new();
    for (name, result) in results {
        let json = serde_json::json!({
            "success": result.success,
            "output": result.output,
            "error": result.error,
        });
        let _ = write!(
            out,
            "<tool_result name=\"{name}\">{}</tool_result>\n",
            serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
        );
    }
    out
}

/// Run the tool-calling loop: call LLM, parse tool calls, execute, feed results back, repeat.
///
/// Returns the final text response (after all tool calls are resolved).
async fn tool_calling_loop(
    provider: &dyn Provider,
    system_prompt: &str,
    initial_message: &str,
    model_name: &str,
    temperature: f64,
    tools: &[Box<dyn Tool>],
) -> Result<String> {
    // Build conversation as alternating user/assistant messages.
    // The provider is stateless, so we pass the full conversation each iteration
    // by concatenating into a single user message (since `chat_with_system` takes one string).
    let mut conversation = initial_message.to_string();
    let mut final_text = String::new();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        let response = provider
            .chat_with_system(Some(system_prompt), &conversation, model_name, temperature)
            .await?;

        let calls = parse_tool_calls(&response);

        // Extract and print any text the LLM produced alongside tool calls
        let text = extract_text_outside_tool_calls(&response);
        if !text.is_empty() {
            if iteration > 0 || !calls.is_empty() {
                // Print intermediate thinking
                eprintln!("{text}");
            }
        }

        if calls.is_empty() {
            // No tool calls — this is the final response
            final_text = response;
            break;
        }

        tracing::debug!(
            iteration,
            num_calls = calls.len(),
            "Executing tool calls"
        );

        let results = execute_tool_calls(tools, &calls).await;

        // Log tool results
        for (name, result) in &results {
            if result.success {
                tracing::debug!(tool = name, "Tool succeeded");
            } else {
                tracing::warn!(
                    tool = name,
                    error = result.error.as_deref().unwrap_or("unknown"),
                    "Tool failed"
                );
            }
        }

        // Build the next conversation turn: original message + assistant response + tool results
        let tool_results_text = format_tool_results(&results);
        let _ = write!(
            conversation,
            "\n\n[Assistant]\n{response}\n\n[Tool Results]\n{tool_results_text}"
        );
    }

    Ok(final_text)
}

/// Build context preamble by searching memory for relevant entries
async fn build_context(mem: &dyn Memory, user_msg: &str) -> String {
    let mut context = String::new();

    // Pull relevant memories for this message
    if let Ok(entries) = mem.recall(user_msg, 5).await {
        if !entries.is_empty() {
            context.push_str("[Memory context]\n");
            for entry in &entries {
                let _ = writeln!(context, "- {}: {}", entry.key, entry.content);
            }
            context.push('\n');
        }
    }

    context
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    config: Config,
    message: Option<String>,
    provider_override: Option<String>,
    model_override: Option<String>,
    temperature: f64,
) -> Result<()> {
    // ── Wire up agnostic subsystems ──────────────────────────────
    let observer: Arc<dyn Observer> =
        Arc::from(observability::create_observer(&config.observability));
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(SecurityPolicy::from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));

    // ── Memory (the brain) ────────────────────────────────────────
    let mem: Arc<dyn Memory> = Arc::from(memory::create_memory(
        &config.memory,
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);
    tracing::info!(backend = mem.name(), "Memory initialized");

    // ── Tools (including memory tools) ────────────────────────────
    let composio_key = if config.composio.enabled {
        config.composio.api_key.as_deref()
    } else {
        None
    };
    let agent_tools = tools::all_tools_with_runtime(
        &security,
        runtime,
        mem.clone(),
        composio_key,
        &config.browser,
    );

    // ── Resolve provider ─────────────────────────────────────────
    let provider_name = provider_override
        .as_deref()
        .or(config.default_provider.as_deref())
        .unwrap_or("openrouter");

    let model_name = model_override
        .as_deref()
        .or(config.default_model.as_deref())
        .unwrap_or("anthropic/claude-sonnet-4-20250514");

    let provider: Box<dyn Provider> = providers::create_routed_provider(
        provider_name,
        config.api_key.as_deref(),
        &config.reliability,
        &config.model_routes,
        model_name,
    )?;

    observer.record_event(&ObserverEvent::AgentStart {
        provider: provider_name.to_string(),
        model: model_name.to_string(),
    });

    // ── Build system prompt from workspace MD files (OpenClaw framework) ──
    let skills = crate::skills::load_skills(&config.workspace_dir);
    let tool_specs: Vec<_> = agent_tools.iter().map(|t| t.spec()).collect();
    let system_prompt = crate::channels::build_system_prompt(
        &config.workspace_dir,
        model_name,
        &tool_specs,
        &skills,
        Some(&config.identity),
    );

    // ── Execute ──────────────────────────────────────────────────
    let start = Instant::now();

    if let Some(msg) = message {
        // Auto-save user message to memory
        if config.memory.auto_save {
            let _ = mem
                .store("user_msg", &msg, MemoryCategory::Conversation)
                .await;
        }

        // Inject memory context into user message
        let context = build_context(mem.as_ref(), &msg).await;
        let enriched = if context.is_empty() {
            msg.clone()
        } else {
            format!("{context}{msg}")
        };

        let response = tool_calling_loop(
            provider.as_ref(),
            &system_prompt,
            &enriched,
            model_name,
            temperature,
            &agent_tools,
        )
        .await?;
        println!("{response}");

        // Auto-save assistant response to daily log
        if config.memory.auto_save {
            let summary = truncate_with_ellipsis(&response, 100);
            let _ = mem
                .store("assistant_resp", &summary, MemoryCategory::Daily)
                .await;
        }
    } else {
        println!("🦀 ZeroClaw Interactive Mode");
        println!("Type /quit to exit.\n");

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let cli = crate::channels::CliChannel::new();

        // Spawn listener
        let listen_handle = tokio::spawn(async move {
            let _ = crate::channels::Channel::listen(&cli, tx).await;
        });

        while let Some(msg) = rx.recv().await {
            // Auto-save conversation turns
            if config.memory.auto_save {
                let _ = mem
                    .store("user_msg", &msg.content, MemoryCategory::Conversation)
                    .await;
            }

            // Inject memory context into user message
            let context = build_context(mem.as_ref(), &msg.content).await;
            let enriched = if context.is_empty() {
                msg.content.clone()
            } else {
                format!("{context}{}", msg.content)
            };

            let response = tool_calling_loop(
                provider.as_ref(),
                &system_prompt,
                &enriched,
                model_name,
                temperature,
                &agent_tools,
            )
            .await?;
            println!("\n{response}\n");

            if config.memory.auto_save {
                let summary = truncate_with_ellipsis(&response, 100);
                let _ = mem
                    .store("assistant_resp", &summary, MemoryCategory::Daily)
                    .await;
            }
        }

        listen_handle.abort();
    }

    let duration = start.elapsed();
    observer.record_event(&ObserverEvent::AgentEnd {
        duration,
        tokens_used: None,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_tool_call() {
        let response = r#"Let me check that. <tool_call>{"name": "shell", "arguments": {"command": "ls"}}</tool_call>"#;
        let calls = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].arguments["command"], "ls");
    }

    #[test]
    fn parse_multiple_tool_calls() {
        let response = r#"<tool_call>{"name": "file_read", "arguments": {"path": "README.md"}}</tool_call>
Also: <tool_call>{"name": "shell", "arguments": {"command": "pwd"}}</tool_call>"#;
        let calls = parse_tool_calls(response);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[1].name, "shell");
    }

    #[test]
    fn parse_no_tool_calls() {
        let response = "Just a plain text response with no tools.";
        let calls = parse_tool_calls(response);
        assert!(calls.is_empty());
    }

    #[test]
    fn parse_malformed_json_skipped() {
        let response = r#"<tool_call>not valid json</tool_call>"#;
        let calls = parse_tool_calls(response);
        assert!(calls.is_empty());
    }

    #[test]
    fn parse_empty_tool_call_tag() {
        let response = "<tool_call></tool_call>";
        let calls = parse_tool_calls(response);
        assert!(calls.is_empty());
    }

    #[test]
    fn parse_missing_name_skipped() {
        let response = r#"<tool_call>{"arguments": {"x": 1}}</tool_call>"#;
        let calls = parse_tool_calls(response);
        assert!(calls.is_empty());
    }

    #[test]
    fn parse_missing_arguments_defaults_to_empty() {
        let response = r#"<tool_call>{"name": "memory_recall"}</tool_call>"#;
        let calls = parse_tool_calls(response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "memory_recall");
        assert!(calls[0].arguments.is_object());
    }

    #[test]
    fn parse_unclosed_tag_ignored() {
        let response = r#"<tool_call>{"name": "shell", "arguments": {}}"#;
        let calls = parse_tool_calls(response);
        assert!(calls.is_empty());
    }

    #[test]
    fn format_tool_results_output() {
        let results = vec![(
            "shell".to_string(),
            crate::tools::ToolResult {
                success: true,
                output: "hello".into(),
                error: None,
            },
        )];
        let formatted = format_tool_results(&results);
        assert!(formatted.contains(r#"<tool_result name="shell">"#));
        assert!(formatted.contains(r#""success":true"#));
        assert!(formatted.contains(r#""output":"hello""#));
        assert!(formatted.contains("</tool_result>"));
    }

    #[test]
    fn format_tool_results_with_error() {
        let results = vec![(
            "file_read".to_string(),
            crate::tools::ToolResult {
                success: false,
                output: String::new(),
                error: Some("permission denied".into()),
            },
        )];
        let formatted = format_tool_results(&results);
        assert!(formatted.contains(r#""success":false"#));
        assert!(formatted.contains("permission denied"));
    }

    #[test]
    fn extract_text_outside_calls() {
        let response = r#"Before <tool_call>{"name":"x","arguments":{}}</tool_call> After"#;
        let text = extract_text_outside_tool_calls(response);
        assert_eq!(text, "Before  After");
    }

    #[test]
    fn extract_text_no_calls() {
        let response = "Just plain text.";
        let text = extract_text_outside_tool_calls(response);
        assert_eq!(text, "Just plain text.");
    }
}
