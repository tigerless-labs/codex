//! Validate the context breakdown against a real captured Codex request.
//!
//! Usage:
//!   cargo run -p codex-tools --example context_breakdown -- \
//!       <instructions.txt> <tools.json> <rollout.jsonl>
//!
//! - instructions.txt : the `instructions` string (base_instructions text)
//! - tools.json       : the request `tools` array (Vec<Value>) as actually sent
//! - rollout.jsonl    : a Codex rollout; `response_item` payloads become `input`
//!
//! Prints the per-source / per-tool / per-tool-output token breakdown so the
//! numbers can be compared to the real request and to the npm `codex-context`.

use std::fs;

use codex_protocol::models::ResponseItem;
use codex_tools::compute_context_breakdown_from_serialized;
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "usage: context_breakdown <instructions.txt> <tools.json> <rollout.jsonl>"
        );
        std::process::exit(2);
    }
    let instructions = fs::read_to_string(&args[1]).expect("read instructions");
    let tools_json: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(&args[2]).expect("read tools")).expect("parse tools");

    // Collect response_item payloads from the rollout into ResponseItem.
    let mut input: Vec<ResponseItem> = Vec::new();
    let mut skipped = 0usize;
    for line in fs::read_to_string(&args[3]).expect("read rollout").lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(payload) = v.get("payload") else { continue };
        match serde_json::from_value::<ResponseItem>(payload.clone()) {
            Ok(item) => input.push(item),
            Err(_) => skipped += 1,
        }
    }

    let b = compute_context_breakdown_from_serialized(&instructions, &tools_json, &input, Some(272_000))
        .expect("breakdown");

    println!("Codex context breakdown (in-process, o200k_base)");
    println!("  response items: {} parsed, {} skipped", input.len(), skipped);
    println!();
    println!("  system prompt : {:>8}", b.system_prompt_tokens);
    println!("  builtin tools : {:>8}", b.builtin_tools_tokens);
    println!("  mcp tools     : {:>8}", b.mcp_tools_tokens);
    println!("  input (msgs)  : {:>8}", b.input_tokens);
    println!("  ----------------------------");
    println!("  total         : {:>8}  / window {:?}", b.total_tokens, b.context_window);
    println!();
    println!("  tools (largest first):");
    for t in b.per_tool.iter().take(20) {
        println!("    {:<32} {:>7}", t.label, t.tokens);
    }
    println!();
    println!("  input by kind:");
    for k in &b.per_input_kind {
        println!("    {:<32} {:>7}  ({} items)", k.label, k.tokens, k.count);
    }
    println!();
    println!("  tool OUTPUT tokens by tool:");
    for t in &b.per_tool_output {
        println!("    {:<32} {:>7}  ({} calls)", t.label, t.tokens, t.count);
    }
}
