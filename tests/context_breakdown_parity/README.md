# Context breakdown parity validation

This folder contains a small root-level validation harness for native `/context`.
It is meant to validate real captured request data, not the TUI.

## What it checks

The validator is a strict request-body source attribution check. It reads an
OpenAI Responses-style request body containing:

- `instructions`
- `tools`
- `input`

For raw JSONL captures, the request body may be the root object or a supported
wrapper such as `request.body`, `body`, or `frame`. Context-xray Codex raw logs
store the real `response.create` request body under `.frame`.

It recomputes the explicit request-body source buckets that correspond to the
native `/context` Context usage view:

- System prompt: `instructions`
- System tools: non-MCP tool definitions in `tools`
- MCP tools: `mcp__...` tool definitions in `tools`
- Skills: native injected skill context fragments found in `input`
- Messages: remaining `input` items
- Used total: the sum of those request-body buckets

It also scans the real request `tools` array for `mcp__...` tool schemas and
can fail when an expected MCP schema is missing. This is the check used to
answer whether MCP schemas are actually included in every captured request,
rather than only appearing in the UI. `--expected-mcp` behavior is unchanged.

## What it does not check

- It does not include output tokens in context source totals.
- It does not treat cache as a context source bucket.
- It does not count arbitrary tool outputs as Skills. Skills means native
  injected skill context fragments only.
- It does not split `message:developer` by source.
- It does not validate Free space. Free space is derived from
  `context_window - Used total`; it is a UI/window-capacity value covered by
  TUI rendering tests, not a raw request-body source bucket.
- It does not use provider usage as the default pass/fail target.

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

The default raw fixture intentionally omits provider usage so the default path
validates request-body parity only.

## Run with a real context-xray log

Use a captured `raw.jsonl` from context-xray or another request recorder, then run:

```bash
bash tests/context_breakdown_parity/run.sh \
  --raw-jsonl ~/.cost-xray/sessions/codex/<session_id>/raw.jsonl \
  --expected-mcp mcp__codex_apps__google_drive \
  --expected-mcp mcp__openaiDeveloperDocs
```

The validator inspects every captured request that contains `instructions`, `tools`, and `input`. If any request is missing one of the expected MCP schemas, the command exits with a non-zero status.

If you have already extracted a request body manually, `--request-json` still
accepts that standalone JSON object:

```bash
bash tests/context_breakdown_parity/run.sh \
  --request-json /tmp/context_xray_request_frame.json
```

## Provider Usage

Provider usage is optional diagnostic output only. It is separate from strict
request-body source attribution because provider `input_tokens` can include
server-side framing, tokenizer, cache, or billing semantics that are not source
buckets in `instructions`, `tools`, and `input`.

To inspect provider usage when a log contains it, pass `--show-provider-usage`:

```bash
bash tests/context_breakdown_parity/run.sh \
  --raw-jsonl tests/context_breakdown_parity/fixtures/raw_with_provider_usage.jsonl \
  --expected-mcp mcp__codex_apps__google_drive \
  --show-provider-usage
```

## Interpreting Deltas

The component table is recomputed from the captured request body using the same
Rust token accounting path as native `/context`. Provider `input_tokens`, when
printed with `--show-provider-usage`, may still differ because provider usage
can include or exclude framing differently and can report cache behavior
separately.

`Messages = 0` can be valid for initial requests whose `input` array is
empty. For stronger validation of current-turn input attribution, also run the
validator against a frame whose `input` array is non-empty.

The pass/fail target is component delta `0` against the parsed request body plus
presence of any expected MCP schemas. It does not prove exact provider billing
or server-side token reconciliation.
