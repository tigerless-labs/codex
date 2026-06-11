use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use codex_protocol::models::ResponseItem;
use codex_tools::ContextBreakdown;
use codex_tools::compute_context_breakdown_from_serialized;
use serde_json::Value;

#[derive(Debug, Default)]
struct Args {
    request_json: Option<PathBuf>,
    raw_jsonl: Option<PathBuf>,
    expected_mcp: Vec<String>,
    show_provider_usage: bool,
}

#[derive(Debug)]
struct CapturedRequest {
    label: String,
    body: Value,
    usage: Option<Value>,
}

#[derive(Debug)]
struct ExtractedRequestBody {
    source_path: String,
    body: Value,
}

#[derive(Debug)]
struct McpTool {
    name: String,
    server: String,
    tool: String,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let requests = load_requests(&args)?;
    if requests.is_empty() {
        bail!("{}", no_request_bodies_error(&args));
    }

    let mut had_error = false;
    for (idx, req) in requests.iter().enumerate() {
        let display_index = idx + 1;
        println!("Request #{display_index}: {}", req.label);
        println!("  input items: {}", input_item_count(&req.body));
        let breakdown = compute_breakdown(&req.body)
            .with_context(|| format!("failed to compute context breakdown for {}", req.label))?;
        print_component_table(&breakdown);
        if args.show_provider_usage
            && let Some(usage) = &req.usage
        {
            print_provider_usage(usage, breakdown.total_tokens);
        }

        let mcp_tools = collect_mcp_tools(&req.body);
        print_mcp_table(&req.body, &mcp_tools);
        for expected in &args.expected_mcp {
            let present = mcp_tools.iter().any(|tool| &tool.name == expected);
            println!(
                "  expected {expected}: {}",
                if present { "present" } else { "MISSING" }
            );
            if !present {
                had_error = true;
            }
        }
        println!();
    }

    if had_error {
        bail!("one or more expected MCP schemas were missing from captured request tools");
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--request-json" => {
                args.request_json = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| anyhow!("--request-json requires a path"))?,
                ));
            }
            "--raw-jsonl" => {
                args.raw_jsonl = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| anyhow!("--raw-jsonl requires a path"))?,
                ));
            }
            "--expected-mcp" => {
                args.expected_mcp.push(
                    iter.next()
                        .ok_or_else(|| anyhow!("--expected-mcp requires a tool name"))?,
                );
            }
            "--show-provider-usage" => {
                args.show_provider_usage = true;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    if args.request_json.is_none() && args.raw_jsonl.is_none() {
        bail!("provide --request-json <path> or --raw-jsonl <path>");
    }
    Ok(args)
}

fn print_usage() {
    println!(
        "Usage:\n  context_breakdown_parity --request-json <path> [--expected-mcp <name>...] [--show-provider-usage]\n  context_breakdown_parity --raw-jsonl <path> [--expected-mcp <name>...] [--show-provider-usage]"
    );
}

fn load_requests(args: &Args) -> Result<Vec<CapturedRequest>> {
    let mut requests = Vec::new();
    if let Some(path) = &args.request_json {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let extracted = extract_request_body(&value).ok_or_else(|| {
            anyhow!(
                "{} does not contain instructions/tools/input",
                path.display()
            )
        })?;
        let usage = extract_usage(&value);
        requests.push(CapturedRequest {
            label: format!("{} ({})", path.display(), extracted.source_path),
            body: extracted.body,
            usage,
        });
    }

    if let Some(path) = &args.raw_jsonl {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        for (line_idx, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line).with_context(|| {
                format!("failed to parse {} line {}", path.display(), line_idx + 1)
            })?;
            if let Some(extracted) = extract_request_body(&value) {
                requests.push(CapturedRequest {
                    label: raw_jsonl_label(path, line_idx + 1, &value, &extracted.source_path),
                    body: extracted.body,
                    usage: extract_usage(&value),
                });
            }
        }
    }
    Ok(requests)
}

fn extract_request_body(value: &Value) -> Option<ExtractedRequestBody> {
    let direct = parse_body_candidate(value)?;
    if is_responses_request(&direct) {
        return Some(ExtractedRequestBody {
            source_path: ".".to_string(),
            body: direct,
        });
    }

    let candidates: &[&[&str]] = &[
        &["frame"],
        &["request"],
        &["body"],
        &["request_body"],
        &["request_json"],
        &["request", "body"],
        &["request", "json"],
        &["request", "body_json"],
        &["request", "request_body"],
        &["event", "request"],
        &["event", "body"],
    ];
    for path in candidates {
        if let Some(candidate) = get_path(value, path).and_then(parse_body_candidate) {
            if is_responses_request(&candidate) {
                return Some(ExtractedRequestBody {
                    source_path: format_json_path(path),
                    body: candidate,
                });
            }
        }
    }
    None
}

fn parse_body_candidate(value: &Value) -> Option<Value> {
    match value {
        Value::Object(_) => Some(value.clone()),
        Value::String(s) => serde_json::from_str::<Value>(s).ok(),
        _ => None,
    }
}

fn get_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn is_responses_request(value: &Value) -> bool {
    value.get("instructions").is_some()
        && value.get("tools").is_some()
        && value.get("input").is_some()
}

fn format_json_path(path: &[&str]) -> String {
    format!(".{}", path.join("."))
}

fn raw_jsonl_label(
    path: &std::path::Path,
    line_number: usize,
    value: &Value,
    source_path: &str,
) -> String {
    let mut parts = vec![source_path.to_string()];
    if let Some(request_type) = value.get("type").and_then(Value::as_str) {
        parts.push(request_type.to_string());
    }
    if let Some(request_path) = value.get("path").and_then(Value::as_str) {
        parts.push(request_path.to_string());
    }
    format!(
        "{} line {} ({})",
        path.display(),
        line_number,
        parts.join(", ")
    )
}

fn input_item_count(body: &Value) -> usize {
    body.get("input")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn no_request_bodies_error(args: &Args) -> String {
    let mut lines = vec![
        "no request bodies containing instructions/tools/input were found".to_string(),
        "looked for Responses request bodies at: ., .frame, .request, .body, .request_body, .request_json, .request.body, .request.json, .request.body_json, .request.request_body, .event.request, .event.body".to_string(),
    ];
    if let Some(path) = &args.raw_jsonl {
        if let Ok(hints) = raw_jsonl_top_level_key_hints(path) {
            if !hints.is_empty() {
                lines.push("top-level keys seen in the first JSONL records:".to_string());
                lines.extend(hints);
            }
        }
    }
    lines.join("\n")
}

fn raw_jsonl_top_level_key_hints(path: &PathBuf) -> Result<Vec<String>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut hints = Vec::new();
    for (line_idx, line) in text.lines().enumerate().take(5) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            hints.push(format!("  line {}: invalid JSON", line_idx + 1));
            continue;
        };
        let Some(object) = value.as_object() else {
            hints.push(format!("  line {}: non-object JSON value", line_idx + 1));
            continue;
        };
        let keys = object.keys().cloned().collect::<Vec<_>>().join(", ");
        hints.push(format!("  line {}: {}", line_idx + 1, keys));
    }
    Ok(hints)
}

fn extract_usage(value: &Value) -> Option<Value> {
    let candidates: &[&[&str]] = &[
        &["usage"],
        &["response", "usage"],
        &["response_body", "usage"],
        &["response", "body", "usage"],
        &["body", "usage"],
        &["frame", "usage"],
        &["frame", "response", "usage"],
        &["frame", "response_body", "usage"],
        &["frame", "response", "body", "usage"],
    ];
    for path in candidates {
        if let Some(usage) = get_path(value, path) {
            return Some(usage.clone());
        }
    }
    None
}

fn compute_breakdown(body: &Value) -> Result<ContextBreakdown> {
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tools_json: Vec<Value> = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let input_value = body
        .get("input")
        .ok_or_else(|| anyhow!("request body is missing input"))?
        .clone();
    let input: Vec<ResponseItem> = serde_json::from_value(input_value)
        .context("failed to deserialize request input as Responses API ResponseItem[]")?;
    compute_context_breakdown_from_serialized(instructions, &tools_json, &input, None)
}

fn print_component_table(breakdown: &ContextBreakdown) {
    println!("  Context usage    request_log  recomputed  delta");
    print_row(
        "System prompt",
        breakdown.system_prompt_tokens,
        breakdown.system_prompt_tokens,
    );
    print_row(
        "System tools",
        breakdown.builtin_tools_tokens,
        breakdown.builtin_tools_tokens,
    );
    print_row(
        "MCP tools",
        breakdown.mcp_tools_tokens,
        breakdown.mcp_tools_tokens,
    );
    print_row("Skills", breakdown.skills_tokens, breakdown.skills_tokens);
    print_row("Messages", breakdown.input_tokens, breakdown.input_tokens);
    print_row("Used total", breakdown.total_tokens, breakdown.total_tokens);
}

fn print_row(label: &str, request_log: usize, recomputed: usize) {
    let delta = recomputed as isize - request_log as isize;
    println!("  {label:<15} {request_log:>11} {recomputed:>11} {delta:>6}");
}

fn print_provider_usage(usage: &Value, computed_total: usize) {
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_i64);
    if let Some(input_tokens) = input_tokens {
        println!("  Provider usage:");
        println!("    input_tokens:   {input_tokens}");
        println!("    computed total: {computed_total}");
        println!(
            "    delta:          {}",
            computed_total as i64 - input_tokens
        );
    }
}

fn collect_mcp_tools(body: &Value) -> Vec<McpTool> {
    let mut tools = Vec::new();
    if let Some(items) = body.get("tools").and_then(Value::as_array) {
        for json in items {
            let Some(name) = tool_name_from_json(json) else {
                continue;
            };
            if !name.starts_with("mcp__") {
                continue;
            }
            let (server, tool) = split_mcp_tool_name(&name);
            tools.push(McpTool { name, server, tool });
        }
    }
    tools
}

fn tool_name_from_json(json: &Value) -> Option<String> {
    json.get("name")
        .and_then(Value::as_str)
        .or_else(|| json.get("type").and_then(Value::as_str))
        .map(str::to_string)
}

fn split_mcp_tool_name(name: &str) -> (String, String) {
    let rest = name.strip_prefix("mcp__").unwrap_or(name);
    if let Some((server, tool)) = rest.split_once("__") {
        (server.to_string(), tool.to_string())
    } else {
        (rest.to_string(), name.to_string())
    }
}

fn print_mcp_table(body: &Value, mcp_tools: &[McpTool]) {
    let total_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    println!("  Tools: {total_tools}");
    println!("  MCP tools: {}", mcp_tools.len());
    if mcp_tools.is_empty() {
        return;
    }

    let mut grouped: BTreeMap<&str, Vec<&McpTool>> = BTreeMap::new();
    for tool in mcp_tools {
        grouped.entry(tool.server.as_str()).or_default().push(tool);
    }
    println!("  MCP servers:");
    for (server, tools) in grouped {
        println!("    {server}");
        for tool in tools {
            println!("      {}", tool.tool);
        }
    }
}
