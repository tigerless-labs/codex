# Native `/context` — implementation

This fork adds an **in-process context-floor token breakdown** to Codex: a
`/context` view that shows exactly what fills the model's context window —
system prompt, tool schemas, and the conversation input (including per-tool
output) — tokenized locally with `o200k_base`.

It answers the request in [openai/codex#13222](https://github.com/openai/codex/issues/13222)
("tokens usage breakdown") and mirrors how Claude Code's `/context` works: call
the real request builders, then count tokens. Nothing is read from rollout logs.

## Approach

At model-request assembly, Codex constructs three things in `build_request`
(`core/src/client.rs`): `instructions` (system prompt), `tools`, and `input`
(conversation items). We tap those exact values and tokenize each:

```
build_prompt (session/turn.rs)
   instructions = base_instructions.text
   tools        = router.model_visible_specs()
   input        = history.for_prompt(...)
        │
        ▼
compute_context_breakdown  (tools/src/context_usage.rs)
   system_prompt_tokens   = o200k(instructions)
   per-tool tokens        = o200k(create_tools_json_for_responses_api(tools)[i])
                            → builtin vs mcp__ connector split
   per-input-kind tokens  = o200k(serialize(item))   grouped by kind
   per-tool-output tokens = outputs attributed to their tool via call_id
        │
        ▼
render: System prompt / Built-in tools / MCP tools / Messages (+ per-tool output)
```

Because the tools are serialized with Codex's own
`create_tools_json_for_responses_api`, the per-tool counts match the bytes
actually sent on the wire. Because `input` is the real `prompt.input` (after
Codex's own history management), the Messages number is the live request — not a
log reconstruction — so the residual is ~0 (only JSON framing).

The token encoder is `o200k_base` via the `tiktoken-rs` crate (local,
deterministic — no API call). This is the one new external dependency.

## Components

| Layer | File | What |
| --- | --- | --- |
| Tokenizer + breakdown | `codex-rs/tools/src/context_usage.rs` | `compute_context_breakdown` (+ `*_from_serialized`), `ContextBreakdown`, `TokenBucket`; unit test; `examples/context_breakdown.rs` |
| Assembly reuse | `codex-rs/core/src/prompt_debug.rs` | `build_prompt_and_window_from_session` → `build_context_breakdown_from_session` |
| Live session method | `codex-rs/core/src/codex_thread.rs` | `CodexThread::context_breakdown()` |
| Exports | `codex-rs/core/src/lib.rs` | `build_context_breakdown`, `ContextBreakdown`, `TokenBucket` |
| CLI | `codex-rs/cli/src/main.rs` | `codex debug prompt-input --tokens` |
| App-server endpoint | `app-server-protocol` (`v2/thread.rs`, `common.rs`), `app-server` (`message_processor.rs`, `request_processors*`) | `thread/context/breakdown` request → response |
| TUI command | `codex-rs/tui/src/{slash_command.rs, chatwidget/slash_dispatch.rs, app_command.rs, app/thread_routing.rs, chatwidget.rs, app_server_session.rs}` | `/context` slash command + scrollback card |

`/context` is a **standalone** command — its own `SlashCommand` variant, its own
app-server endpoint, and its own rendering. It shares no code or data path with
`/status` (which was only used as a wiring template).

## Surfaces

```bash
# CLI (no TUI session needed; assembles a debug turn)
codex debug prompt-input --tokens

# TUI: type inside a session
/context
```

App-server clients can call the `thread/context/breakdown` method with
`{ threadId }` and receive `ContextBreakdownResponse`.

## What it reports

- `system_prompt_tokens`, `builtin_tools_tokens`, `mcp_tools_tokens`,
  `input_tokens`, `total_tokens`, `context_window`.
- `per_tool` — each tool definition, largest first (built-in vs `mcp__*`).
- `per_input_kind` — `message:user` / `reasoning` / `tool call` / `tool output` / …
- `per_tool_output` — tool outputs grouped by the tool that produced them
  (attributed via `call_id` → name). This is the part logs and external assembly
  cannot get exactly.

## Validation

On a live Codex 0.135.0 session:

- System prompt is **byte-identical** to the real request (4,376 tokens).
- Per-tool sums match the whole serialized tools array within **0.06%**.
- Example live breakdown: system 4,376 · built-in 2,489 · mcp/connector 17,033 ·
  input 26,662 = **50,560 / 272k**, with `exec_command` output alone **18,368
  tokens across 22 calls**.

## Build & run

This dev machine lacks `pkg-config` (a transitive `openssl-sys` dep), but
`libssl-dev` is installed, so point the build at it:

```bash
export OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu \
       OPENSSL_INCLUDE_DIR=/usr/include \
       OPENSSL_NO_VENDOR=1
cargo test -p codex-tools context_usage::      # unit test
cargo build -p codex-cli --bin codex           # full binary (Rust 1.95.0)
./codex-rs/target/debug/codex                  # then type /context
```

## Limitations

- The breakdown reflects the assembled request for the current/next turn. The
  in-process path is exact for that; an external tool reading the rollout (or a
  hook's `transcript_path`) can get per-tool output exactly too, but not the
  post-truncation live occupancy.
- Tool schemas are not version-pinned here (unlike the external `codex-context`
  tool) because we read the live `tools` directly — always matching the running
  build.
