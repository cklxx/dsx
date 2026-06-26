<h1 align="center">🐳 dsx</h1>

<p align="center"><strong>dsx</strong> is a terminal coding agent powered by <strong>DeepSeek&nbsp;V4</strong> that runs locally on your machine.</p>

<p align="center"><code>&gt;_ dsx</code></p>

---

dsx lives in your terminal and works alongside you on real code: it reads the repo, runs commands in a sandbox, edits files, searches the web, and reasons out loud — driven entirely by DeepSeek V4. It's local-first and API-key only: no account, no login, no cloud session.

## Highlights

- **DeepSeek V4, end to end.** `deepseek-v4-pro` runs your tasks; `deepseek-v4-flash` handles auto-summaries, context compaction, and side work. Routing between them is automatic.
- **Live reasoning.** Talks the DeepSeek Anthropic-compatible Messages wire and streams `<think>` reasoning as it works.
- **Built-in web.** A `web_search` tool (keyless DuckDuckGo by default, pluggable backend) and a `read_url` tool that fetches a page and extracts readable text — so the agent can look things up mid-task.
- **Agentic by default.** Plans, runs shell commands under a sandbox with approval controls, applies edits, and verifies its own work before handing back.
- **Made to feel like dsx.** A spouting-whale welcome screen and a DeepSeek-blue UI, in your terminal.
- **Local & private.** Just a `DEEPSEEK_API_KEY` in your environment; nothing else leaves your machine.

## Quickstart

dsx is a Rust workspace — build the binary from source:

```shell
git clone git@github.com:cklxx/dsx.git
cd dsx/codex-rs
cargo build --release --bin dsx
```

Set your DeepSeek API key (get one at [platform.deepseek.com](https://platform.deepseek.com)) and run it:

```shell
export DEEPSEEK_API_KEY=sk-...
./target/release/dsx
```

That drops you into the interactive TUI. For a quick one-shot instead:

```shell
DEEPSEEK_API_KEY=sk-... cargo run -q -p codex-exec -- "explain what this repo does"
```

### Put `dsx` on your PATH

```shell
ln -sf "$PWD/target/release/dsx" ~/.local/bin/dsx   # or ~/.cargo/bin/dsx
```

Then just `dsx` from anywhere.

## Configuration

Config lives in `$CODEX_HOME/config.toml` (default `~/.codex/config.toml`; point `CODEX_HOME` elsewhere to keep a dedicated dsx home):

```toml
model          = "deepseek-v4-pro"   # or deepseek-v4-flash
model_provider = "deepseek"           # the only built-in provider
```

The `deepseek` provider is the default and reads `DEEPSEEK_API_KEY` from the environment — there is no login flow. Switch models in-session with `/model`.

## How it works

dsx is layered: a core engine, a headless server (`dsx app-server`, JSON-RPC over `stdio`/`unix`/`ws`), and front-ends that drive it — so a native UI can sit on the same core. Key pieces:

- DeepSeek Anthropic wire — `codex-rs/codex-api/src/{anthropic.rs, sse/anthropic.rs, endpoint/anthropic.rs}`
- Provider + model catalog — `codex-rs/model-provider-info/` and `codex-rs/models-manager/models.json`
- Web tools — `codex-rs/core/src/tools/web.rs`

## License

Built on [OpenAI Codex](https://github.com/openai/codex) and licensed under [Apache-2.0](LICENSE). DeepSeek and the DeepSeek V4 models are products of DeepSeek; dsx is an independent, unofficial client.
