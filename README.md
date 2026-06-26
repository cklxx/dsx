<p align="center"><strong>dsx</strong> is a terminal coding agent powered by <strong>DeepSeek&nbsp;V4</strong> that runs locally on your computer.</p>

<p align="center">
  <code>🐳 &gt;_ dsx</code> — DeepSeek-only fork of OpenAI Codex.
</p>

---

## What it is

`dsx` is a fork of [OpenAI Codex CLI](https://github.com/openai/codex), retargeted to speak **only** the DeepSeek V4 API. The OpenAI Responses wire, ChatGPT login, and hosted tools are removed; in their place dsx talks to DeepSeek's Anthropic-compatible Messages API and ships its own web tools.

- **Models** — `deepseek-v4-pro` (default, used for normal task execution) and `deepseek-v4-flash` (fast model used for auto-summaries, context compaction, and auxiliary tasks).
- **Wire** — DeepSeek's Anthropic-compatible endpoint (`https://api.deepseek.com/anthropic/v1/messages`), authenticated with `x-api-key`. Streaming reasoning (`<think>`) is rendered live.
- **Web** — built-in `web_search` (keyless DuckDuckGo by default, pluggable backend) and `read_url` (fetch + HTML→text) tools the agent can call.
- **Local-first** — API-key only, no account or login flow.

## Quickstart

dsx is a Rust workspace; build the binary from source:

```shell
git clone git@github.com:cklxx/dsx.git
cd dsx/codex-rs
cargo build --release --bin dsx
```

Set your DeepSeek API key and run it:

```shell
export DEEPSEEK_API_KEY=sk-...        # get one at https://platform.deepseek.com
./target/release/dsx
```

Get a key at [platform.deepseek.com](https://platform.deepseek.com). The default model is `deepseek-v4-pro`; switch in-session with `/model` or pin it in config.

### One-shot (non-interactive)

```shell
DEEPSEEK_API_KEY=sk-... cargo run -q -p codex-exec -- "summarize the changes in this repo"
```

## Configuration

Config lives in `$CODEX_HOME/config.toml` (default `~/.codex/config.toml`):

```toml
model          = "deepseek-v4-pro"   # or deepseek-v4-flash
model_provider = "deepseek"           # the only built-in provider
```

The `deepseek` provider is the default and reads `DEEPSEEK_API_KEY` from the environment. flash↔pro routing is automatic: normal turns run on `deepseek-v4-pro`; compaction, auto-summaries, and side tasks run on `deepseek-v4-flash`.

## Architecture

The engine, headless server (`dsx app-server`, JSON-RPC), and front-ends are layered, so a native UI can drive the same core over `unix://`/`ws://`. The DeepSeek Anthropic wire lives in `codex-rs/codex-api/src/{anthropic.rs, sse/anthropic.rs, endpoint/anthropic.rs}`; the provider + catalog in `codex-rs/model-provider-info` and `codex-rs/models-manager/models.json`; the web tools in `codex-rs/core/src/tools/web.rs`.

## Credits & License

dsx is built on [OpenAI Codex](https://github.com/openai/codex) and is licensed under the [Apache-2.0 License](LICENSE). DeepSeek and the DeepSeek V4 models are products of DeepSeek; this project is an independent, unofficial client.
