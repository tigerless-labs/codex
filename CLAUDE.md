# CLAUDE.md

Guidance for AI agents (and humans) working in this fork.

## What this fork adds

A native **`/context`** command: an in-process context-floor token breakdown
(system prompt + tool schemas + conversation input, incl. per-tool output),
tokenized locally with `o200k_base`. It implements
[openai/codex#13222](https://github.com/openai/codex/issues/13222).

**Read this first:** [`docs/native-context.md`](docs/native-context.md) — the
full implementation (where it taps the request assembly, the files involved, the
three surfaces, and validation).

## Where the code lives

- `codex-rs/tools/src/context_usage.rs` — tokenizer + `compute_context_breakdown`
- `codex-rs/core/src/prompt_debug.rs`, `codex-rs/core/src/codex_thread.rs` — live-session assembly
- `codex-rs/cli/src/main.rs` — `codex debug prompt-input --tokens`
- `codex-rs/app-server*` — `thread/context/breakdown` endpoint
- `codex-rs/tui/...` — `/context` slash command + card

`/context` is standalone — it does **not** share code with `/status`.

## Build & test

This machine has no `pkg-config`; `libssl-dev` is present, so set the OpenSSL
paths before building (Rust pinned to 1.95.0 via `rust-toolchain.toml`):

```bash
export OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu \
       OPENSSL_INCLUDE_DIR=/usr/include \
       OPENSSL_NO_VENDOR=1

cargo test -p codex-tools context_usage::      # unit test
cargo build -p codex-cli --bin codex           # full binary
./codex-rs/target/debug/codex                  # then type /context
```

Before pushing, run `cargo fmt` and `cargo clippy` (upstream CI enforces both).

## Conventions

- Commits in this fork are authored by the repo owner; **do not add any
  `Co-Authored-By` / AI attribution trailer** to commit messages.
- Keep `/context` independent of `/status`.
