# AgentBanter

A readable dashboard for **Codex, ChatGPT, and Claude Code** conversations: prompts, replies, available reasoning summaries, tool steps, and decisions.

AgentBanter is the renamed and expanded version of claude-progress. It runs as one Rust binary with its HTML, CSS, and JavaScript included.

| Source | How it works | Updates |
|---|---|---|
| Codex desktop / CLI | Reads local rollout JSONL under `$CODEX_HOME/sessions` (default `~/.codex/sessions`) | Live |
| ChatGPT | Imports `conversations.json` from an unzipped ChatGPT data export | Snapshot; import a newer export to update |
| Claude Code | Reads `~/.claude/projects` | Live |

**ChatGPT imports are not a live connection to the ChatGPT app.** Cloud conversations without local rollout logs need an export. AgentBanter does not access app credentials or private cloud endpoints. Encrypted reasoning is not available; only plaintext reasoning or summaries actually present in the source can be shown. Image and other non-text attachments in exports appear as placeholders.

## Run

Requires Rust 1.85 or newer.

```sh
cargo run --release -- --open
# http://127.0.0.1:7331/latest
```

Install from this checkout:

```sh
cargo install --path .
agentbanter --open
```

All three sources are enabled by default; absent directories are skipped. The home page lists sessions grouped by project and labels their source. To show only Codex and ChatGPT:

```sh
agentbanter --sources codex,chatgpt --open
```

### ChatGPT conversations

Unzip a ChatGPT data export and import its `conversations.json`:

```sh
agentbanter --import-chatgpt /path/to/conversations.json
agentbanter --sources chatgpt,codex --open
```

The import command saves transcripts to the OS local data directory under `agentbanter/chatgpt` (`~/.local/share/agentbanter/chatgpt` on Linux), then exits. Use `--chatgpt-dir` on both commands to choose another location. Single-conversation JSON objects with the same `mapping` format are also accepted.

The importer follows `current_node` back through its parents, displaying the selected conversation branch rather than mixing regenerated answers. If `current_node` is missing, it selects the newest leaf. System messages and hidden messages are omitted. Re-importing a conversation replaces its snapshot; open pages reload when that snapshot changes. Identical imports leave files untouched. Importing does not delete conversations missing from the newer export.

### Codex sessions

Desktop and CLI coding sessions with local rollout files are discovered recursively, including the year/month/day directory layout. Prompts, replies, plaintext reasoning summaries, function/custom tool calls, and outputs are translated into the shared timeline. Mirrored `event_msg` records are ignored to avoid duplicate messages. A remote session needs its rollout files available locally, for example through a mounted directory:

```sh
agentbanter --sources codex --codex-dir /mounted/codex/sessions
```

Codex and ChatGPT session URLs are prefixed with their source so their IDs cannot collide with Claude IDs. Sending messages from AgentBanter is currently supported only for Claude Code.

## Dashboard

- Numbered prompts with duration, commands, files touched, decisions, and failure counts.
- Expandable reasoning summaries and Markdown replies.
- Grouped tool calls with inline results; command and patch summaries for Codex tools.
- A decisions panel combining explicit Claude question answers, decision documents, and clearly marked heuristic inferences from prose.
- Toggles for reasoning, tools, and inferred decisions; follow mode to scroll with new activity.
- Live updates over server-sent events, reconnect support, responsive layout, and OS light/dark themes.

Subagent transcripts are not nested under their parent call. Tool detail and file tracking depend on the tool's recorded input format; nested orchestration scripts appear as one tool call.

## Options

```text
agentbanter [OPTIONS]

--sources <SOURCES>             Comma-separated claude,codex,chatgpt (default: all)
--projects-dir <DIR>            Claude transcript root (default: ~/.claude/projects)
--codex-dir <DIR>               Codex rollout root (default: $CODEX_HOME/sessions)
--chatgpt-dir <DIR>             Imported ChatGPT transcript directory
--import-chatgpt <JSON>         Import ChatGPT JSON, then exit
--port <PORT>                  Port (default: 7331)
--bind <IP>                    Repeatable listen address
--poll-ms <MS>                 Transcript polling interval (default: 750)
--open                         Open the latest session in the browser
--allow-send                   Enable the Claude-only message composer
--claude-bin <PATH>            Claude sending binary (default: claude)
--send-permission-mode <MODE>  Claude sending permission mode (default: auto)
--templates <DIR>              Override built-in templates; hot-reloaded
--static <DIR>                 Override built-in static assets
--dump-assets <DIR>            Extract templates and assets, then exit
```

Routes: `/` lists sessions, `/latest` opens the most recently modified session, `/s/<id>` shows a session, and `/s/<id>/events` streams updates. `/s/<id>/send` exposes Claude sending/status when enabled.

### Listen addresses and sending

By default, AgentBanter listens on `127.0.0.1` and the machine's Tailscale IPv4, if available. Use `--bind 127.0.0.1` for local-only access. It makes no model API requests for viewing or importing transcripts. Transcripts can contain sensitive text, and everyone with access to the dashboard can read them.

`--allow-send` adds a Claude message box that runs `claude -p --resume <id>` in the session directory. Sessions held by another process or recently written elsewhere are forked to avoid splitting an active chat. Headless runs use `--send-permission-mode`. Anyone with dashboard access can run Claude as you when this is enabled. Cross-origin browser sends are refused. Codex and ChatGPT sessions remain read-only even with this option.

### Beside the chat

Open the dashboard in a browser window, VS Code Simple Browser, or the ChatGPT/Codex app's built-in browser at `http://127.0.0.1:7331/`.

### Linux user service

```ini
# ~/.config/systemd/user/agentbanter.service
[Unit]
Description=AgentBanter

[Service]
ExecStart=%h/.cargo/bin/agentbanter
Restart=on-failure

[Install]
WantedBy=default.target
```

```sh
systemctl --user enable --now agentbanter
```

## Customize

```sh
agentbanter --dump-assets ~/.config/agentbanter
agentbanter --templates ~/.config/agentbanter/templates --static ~/.config/agentbanter/static
```

Templates use MiniJinja; design tokens are in `static/app.css`. Initial HTML and live fragments use the same partials. Running in a checkout automatically uses its `templates/` and `static/` directories.

## Development

```sh
# Synthetic examples; no personal conversation data needed
cargo run -- --sources claude,codex,chatgpt --projects-dir fixtures/demo --codex-dir fixtures/codex --chatgpt-dir /tmp/agentbanter-demo-chatgpt --open
# In a second terminal, add the ChatGPT example
cargo run -- --import-chatgpt fixtures/chatgpt/conversations.json --chatgpt-dir /tmp/agentbanter-demo-chatgpt

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- `src/source.rs`: source identification and Codex record normalization.
- `src/chatgpt.rs`: ChatGPT export validation, branch selection, and import.
- `src/store.rs`: session indexing, lazy parsing, and polling.
- `src/model.rs`: timeline events and statistics.
- `src/tools.rs`: tool summaries and touched-file extraction.
- `src/main.rs`: CLI, HTTP routes, and live streaming.
- `templates/` and `static/`: dashboard UI, embedded through `src/assets.rs`.

Transcript schemas can change; unknown record types are ignored and malformed lines are counted. New templates or static files must be added to `src/assets.rs`.

## License

[MIT](LICENSE) © 2026 Eugene Trotsan
