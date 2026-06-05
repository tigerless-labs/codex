# Context breakdown parity validation

This folder contains a small root-level validation harness for native `/context`.
It is meant to validate real captured request data, not the TUI.

## What it checks

The validator reads an OpenAI Responses-style request body containing:

- `instructions`
- `tools`
- `input`

It recomputes the same context source buckets used by native `/context`:

- instructions / system prompt tokens
- built-in tool definition tokens
- MCP tool definition tokens
- input/messages tokens
- total context/input tokens

It also scans the real request `tools` array for `mcp__...` tool schemas and can fail when an expected MCP schema is missing. This is the check used to answer whether MCP schemas are actually included in every captured request, rather than only appearing in the UI.

## What it does not check

- It does not include output tokens in context source totals.
- It does not treat cache as a context source bucket.
- Provider usage, if present in logs, is printed separately as a sanity check only.
- It does not split `message:developer` by source.

## Run with fixture data

From the repository root:

```bash
bash tests/context_breakdown_parity/run.sh \
  --request-json tests/context_breakdown_parity/fixtures/openai_responses_request.json \
  --expected-mcp mcp__codex_apps__google_drive \
  --expected-mcp mcp__openaiDeveloperDocs
```

You can also validate a raw JSONL fixture:

```bash
bash tests/context_breakdown_parity/run.sh \
  --raw-jsonl tests/context_breakdown_parity/fixtures/raw.jsonl \
  --expected-mcp mcp__codex_apps__google_drive \
  --expected-mcp mcp__openaiDeveloperDocs
```

## Run with a real context-xray log

Use a captured `raw.jsonl` from context-xray or another request recorder, then run:

```bash
bash tests/context_breakdown_parity/run.sh \
  --raw-jsonl /path/to/raw.jsonl \
  --expected-mcp mcp__codex_apps__google_drive \
  --expected-mcp mcp__openaiDeveloperDocs
```

The validator inspects every captured request that contains `instructions`, `tools`, and `input`. If any request is missing one of the expected MCP schemas, the command exits with a non-zero status.

## Interpreting deltas

The component table is recomputed from the captured request body using the same Rust token accounting path as native `/context`. Provider `input_tokens`, if available, may still differ because provider usage can include or exclude framing differently and can report cache behavior separately.
