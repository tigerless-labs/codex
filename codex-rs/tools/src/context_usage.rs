//! In-process context-floor token breakdown — the data behind a native
//! `/context` command.
//!
//! Mirrors Claude Code's `getContextUsage`: take the exact components the model
//! request is assembled from (`instructions`, `tools`, `input`) and tokenize
//! each with the model's real encoding (`o200k_base`). Unlike Claude, Codex can
//! tokenize fully locally — `o200k_base` is OpenAI's published encoding.
//!
//! The tool serialization here is the *same* one the wire request uses
//! ([`crate::create_tools_json_for_responses_api`]), so per-tool counts match
//! what is actually sent. Input items are serialized via their `ResponseItem`
//! serde representation, which is the wire shape too.

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::HashMap;

use codex_protocol::models::ResponseItem;
use serde::Serialize;
use serde_json::Value;

use crate::tool_spec::ToolSpec;
use crate::tool_spec::create_tools_json_for_responses_api;

/// A named token bucket (a source, a tool, or an input kind).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TokenBucket {
    pub label: String,
    pub tokens: usize,
    /// Number of items folded into this bucket.
    pub count: usize,
}

/// The assembled context-floor breakdown for one turn.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextBreakdown {
    /// `instructions` (base system prompt) tokens.
    pub system_prompt_tokens: usize,
    /// Built-in tool definitions (non-`mcp__` tools).
    pub builtin_tools_tokens: usize,
    /// MCP / connector tool definitions (`mcp__*`).
    pub mcp_tools_tokens: usize,
    /// One entry per tool definition, largest first.
    pub per_tool: Vec<TokenBucket>,
    /// Total tokens for the conversation `input` items.
    pub input_tokens: usize,
    /// Input grouped by kind (`message:user`, `reasoning`, `tool call`, …).
    pub per_input_kind: Vec<TokenBucket>,
    /// Tool *outputs* grouped by the tool that produced them.
    pub per_tool_output: Vec<TokenBucket>,
    /// `system_prompt + builtin + mcp + input` — the full request size.
    pub total_tokens: usize,
    /// Resolved model context window, if known.
    pub context_window: Option<i64>,
}

/// One native-vs-request token validation row.
#[cfg(test)]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextBreakdownValidationRow {
    pub component: String,
    pub native: usize,
    pub request: usize,
    pub delta: isize,
}

/// One native-vs-request MCP tool validation row.
#[cfg(test)]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpToolBreakdownValidationRow {
    pub server: String,
    pub tool: String,
    pub native: usize,
    pub request: usize,
    pub delta: isize,
}

/// Validation that native `/context` accounting matches request-shaped data.
#[cfg(test)]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextBreakdownValidation {
    pub components: Vec<ContextBreakdownValidationRow>,
    pub mcp_tools: Vec<McpToolBreakdownValidationRow>,
}

/// Count `o200k_base` tokens. Builds the encoder once per call; callers doing
/// many breakdowns should prefer [`Counter`].
fn count_with(bpe: &tiktoken_rs::CoreBPE, text: &str) -> usize {
    bpe.encode_ordinary(text).len()
}

/// Tool name from its serialized JSON: `name` for function/namespace/custom,
/// else `type` for hosted tools (`web_search`, `image_generation`).
fn tool_name_from_json(json: &Value) -> String {
    json.get("name")
        .and_then(Value::as_str)
        .or_else(|| json.get("type").and_then(Value::as_str))
        .unwrap_or("(unnamed)")
        .to_string()
}

fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp__")
}

#[cfg(test)]
fn split_mcp_tool_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    rest.split_once("__")
}

/// Compute the breakdown from a [`Prompt`](crate)'s components.
///
/// `tools` is serialized with [`create_tools_json_for_responses_api`] so the
/// counts equal the wire request exactly.
pub fn compute_context_breakdown(
    instructions: &str,
    tools: &[ToolSpec],
    input: &[ResponseItem],
    context_window: Option<i64>,
) -> anyhow::Result<ContextBreakdown> {
    let tools_json = create_tools_json_for_responses_api(tools)?;
    compute_context_breakdown_from_serialized(instructions, &tools_json, input, context_window)
}

/// Same as [`compute_context_breakdown`] but takes already-serialized tool JSON
/// (the output of [`create_tools_json_for_responses_api`], or a captured
/// request). Enables validation against real request payloads.
pub fn compute_context_breakdown_from_serialized(
    instructions: &str,
    tools_json: &[Value],
    input: &[ResponseItem],
    context_window: Option<i64>,
) -> anyhow::Result<ContextBreakdown> {
    let bpe = tiktoken_rs::o200k_base()?;

    let system_prompt_tokens = count_with(&bpe, instructions);

    // --- tools ---
    let mut per_tool = Vec::with_capacity(tools_json.len());
    let mut builtin_tools_tokens = 0usize;
    let mut mcp_tools_tokens = 0usize;
    for json in tools_json {
        let name = tool_name_from_json(json);
        let tokens = count_with(&bpe, &serde_json::to_string(json)?);
        if is_mcp_tool(&name) {
            mcp_tools_tokens += tokens;
        } else {
            builtin_tools_tokens += tokens;
        }
        per_tool.push(TokenBucket {
            label: name,
            tokens,
            count: 1,
        });
    }
    per_tool.sort_by_key(|b| std::cmp::Reverse(b.tokens));

    // --- input items ---
    // First map call_id -> tool name so outputs can be attributed.
    let mut call_names: HashMap<&str, &str> = HashMap::new();
    for item in input {
        match item {
            ResponseItem::FunctionCall { call_id, name, .. } => {
                call_names.insert(call_id.as_str(), name.as_str());
            }
            ResponseItem::CustomToolCall { call_id, name, .. } => {
                call_names.insert(call_id.as_str(), name.as_str());
            }
            _ => {}
        }
    }

    let mut kind: HashMap<String, (usize, usize)> = HashMap::new();
    let mut tool_out: HashMap<String, (usize, usize)> = HashMap::new();
    let mut input_tokens = 0usize;
    for item in input {
        let tokens = count_with(&bpe, &serde_json::to_string(item)?);
        input_tokens += tokens;

        let label = match item {
            ResponseItem::Message { role, .. } => format!("message:{role}"),
            ResponseItem::Reasoning { .. } => "reasoning".to_string(),
            ResponseItem::FunctionCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::ToolSearchCall { .. } => "tool call".to_string(),
            ResponseItem::FunctionCallOutput { call_id, .. }
            | ResponseItem::CustomToolCallOutput { call_id, .. } => {
                let name = call_names
                    .get(call_id.as_str())
                    .copied()
                    .unwrap_or("(unknown)");
                let e = tool_out.entry(name.to_string()).or_insert((0, 0));
                e.0 += tokens;
                e.1 += 1;
                "tool output".to_string()
            }
            ResponseItem::ToolSearchOutput { .. } => "tool output".to_string(),
            ResponseItem::WebSearchCall { .. } => "web_search".to_string(),
            ResponseItem::ImageGenerationCall { .. } => "image_generation".to_string(),
            ResponseItem::Compaction { .. }
            | ResponseItem::CompactionTrigger
            | ResponseItem::ContextCompaction { .. } => "compaction".to_string(),
            ResponseItem::Other => "other".to_string(),
        };
        let e = kind.entry(label).or_insert((0, 0));
        e.0 += tokens;
        e.1 += 1;
    }

    let per_input_kind = into_sorted_buckets(kind);
    let per_tool_output = into_sorted_buckets(tool_out);

    let total_tokens =
        system_prompt_tokens + builtin_tools_tokens + mcp_tools_tokens + input_tokens;

    Ok(ContextBreakdown {
        system_prompt_tokens,
        builtin_tools_tokens,
        mcp_tools_tokens,
        per_tool,
        input_tokens,
        per_input_kind,
        per_tool_output,
        total_tokens,
        context_window,
    })
}

/// Compare native `/context` accounting against the same request components
/// after they have been shaped as a Responses API request.
#[cfg(test)]
pub fn validate_context_breakdown_against_serialized_request(
    instructions: &str,
    tools: &[ToolSpec],
    request_tools_json: &[Value],
    input: &[ResponseItem],
    context_window: Option<i64>,
) -> anyhow::Result<ContextBreakdownValidation> {
    let native = compute_context_breakdown(instructions, tools, input, context_window)?;
    let request = compute_context_breakdown_from_serialized(
        instructions,
        request_tools_json,
        input,
        context_window,
    )?;

    let components = vec![
        validation_row(
            "instructions",
            native.system_prompt_tokens,
            request.system_prompt_tokens,
        ),
        validation_row(
            "built-in tool definitions",
            native.builtin_tools_tokens,
            request.builtin_tools_tokens,
        ),
        validation_row(
            "MCP tool definitions",
            native.mcp_tools_tokens,
            request.mcp_tools_tokens,
        ),
        validation_row("messages/input", native.input_tokens, request.input_tokens),
        validation_row("total", native.total_tokens, request.total_tokens),
    ];

    let request_mcp_tokens: BTreeMap<_, _> = request
        .per_tool
        .iter()
        .filter(|bucket| is_mcp_tool(&bucket.label))
        .map(|bucket| (bucket.label.as_str(), bucket.tokens))
        .collect();
    let mut mcp_tools = native
        .per_tool
        .iter()
        .filter_map(|bucket| {
            let (server, tool) = split_mcp_tool_name(&bucket.label)?;
            let request_tokens = request_mcp_tokens
                .get(bucket.label.as_str())
                .copied()
                .unwrap_or(0);
            Some(McpToolBreakdownValidationRow {
                server: server.to_string(),
                tool: tool.to_string(),
                native: bucket.tokens,
                request: request_tokens,
                delta: token_delta(bucket.tokens, request_tokens),
            })
        })
        .collect::<Vec<_>>();
    mcp_tools.sort_by(|a, b| a.server.cmp(&b.server).then_with(|| a.tool.cmp(&b.tool)));

    Ok(ContextBreakdownValidation {
        components,
        mcp_tools,
    })
}

#[cfg(test)]
fn validation_row(component: &str, native: usize, request: usize) -> ContextBreakdownValidationRow {
    ContextBreakdownValidationRow {
        component: component.to_string(),
        native,
        request,
        delta: token_delta(native, request),
    }
}

#[cfg(test)]
fn token_delta(native: usize, request: usize) -> isize {
    native as isize - request as isize
}

fn into_sorted_buckets(map: HashMap<String, (usize, usize)>) -> Vec<TokenBucket> {
    let mut v: Vec<TokenBucket> = map
        .into_iter()
        .map(|(label, (tokens, count))| TokenBucket {
            label,
            tokens,
            count,
        })
        .collect();
    v.sort_by_key(|b| std::cmp::Reverse(b.tokens));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::FunctionCallOutputPayload;
    use pretty_assertions::assert_eq;

    fn func_call(call_id: &str, name: &str, args: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: None,
            arguments: args.to_string(),
            call_id: call_id.to_string(),
        }
    }

    fn func_output(call_id: &str, out: &str) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(out.to_string()),
                success: Some(true),
            },
        }
    }

    fn function_tool(name: &str, description: &str) -> ToolSpec {
        ToolSpec::Function(crate::ResponsesApiTool {
            name: name.to_string(),
            description: description.to_string(),
            strict: false,
            defer_loading: None,
            parameters: crate::JsonSchema::default(),
            output_schema: None,
        })
    }

    #[test]
    fn categorizes_tools_and_attributes_outputs() {
        let instructions = "You are Codex.";
        // A builtin function tool and an MCP/connector tool.
        let tools_json = vec![
            serde_json::json!({
                "type": "function",
                "name": "exec_command",
                "description": "Runs a command.",
                "parameters": {"type": "object", "properties": {}}
            }),
            serde_json::json!({
                "type": "function",
                "name": "mcp__github__fetch_pr",
                "description": "Fetch a PR.",
                "parameters": {"type": "object", "properties": {}}
            }),
        ];
        let input = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![],
                phase: None,
            },
            func_call("call_1", "exec_command", "{\"cmd\":\"ls\"}"),
            func_output("call_1", "file_a\nfile_b\n"),
        ];

        let b = compute_context_breakdown_from_serialized(
            instructions,
            &tools_json,
            &input,
            Some(272_000),
        )
        .expect("breakdown");

        assert!(b.system_prompt_tokens > 0);
        assert!(
            b.builtin_tools_tokens > 0,
            "exec_command counted as builtin"
        );
        assert!(b.mcp_tools_tokens > 0, "mcp__ tool counted as MCP");
        assert_eq!(b.per_tool.len(), 2);
        // The exec_command output is attributed to exec_command.
        assert_eq!(b.per_tool_output.len(), 1);
        assert_eq!(b.per_tool_output[0].label, "exec_command");
        assert!(b.per_tool_output[0].tokens > 0);
        assert_eq!(
            b.total_tokens,
            b.system_prompt_tokens + b.builtin_tools_tokens + b.mcp_tools_tokens + b.input_tokens
        );
    }

    #[test]
    fn validates_native_breakdown_against_serialized_request_tools() {
        let instructions = "You are Codex.";
        let tools = vec![
            function_tool("exec_command", "Runs a command."),
            function_tool("mcp__codex_apps__google_drive", "Access Google Drive."),
        ];
        let request_tools_json =
            create_tools_json_for_responses_api(&tools).expect("serialize request tools");
        let input = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![],
                phase: None,
            },
            func_call("call_1", "mcp__codex_apps__google_drive", "{}"),
        ];

        let validation = validate_context_breakdown_against_serialized_request(
            instructions,
            &tools,
            &request_tools_json,
            &input,
            Some(272_000),
        )
        .expect("validation");

        assert_eq!(
            validation
                .components
                .iter()
                .map(|row| (row.component.as_str(), row.delta))
                .collect::<Vec<_>>(),
            vec![
                ("instructions", 0),
                ("built-in tool definitions", 0),
                ("MCP tool definitions", 0),
                ("messages/input", 0),
                ("total", 0),
            ]
        );
        assert_eq!(
            validation
                .mcp_tools
                .iter()
                .map(|row| (
                    row.server.as_str(),
                    row.tool.as_str(),
                    row.native,
                    row.request,
                    row.delta
                ))
                .collect::<Vec<_>>(),
            vec![(
                "codex_apps",
                "google_drive",
                validation.mcp_tools[0].native,
                validation.mcp_tools[0].native,
                0
            )]
        );
    }
}
