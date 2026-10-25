# AgentBanter

A readable dashboard for **Codex, ChatGPT, and Claude Code** conversations: prompts, replies, available reasoning summaries, tool steps, and decisions.

AgentBanter is the renamed and expanded version of claude-progress. It runs as one Rust binary with its HTML, CSS, and JavaScript included.

| Source | How it works | Updates |
|---|---|---|
| Codex desktop / CLI | Reads local rollout JSONL under `$CODEX_HOME/sessions` (default `~/.codex/sessions`) | Live |
| ChatGPT | Imports `conversations.json` from an unzipped ChatGPT data export | Snapshot; import a newer export to update |
| Claude Code | Reads `~/.claude/projects` | Live |

**ChatGPT imports are not a live connection to the ChatGPT app.** Cloud conversations without local rollout logs need an export. AgentBanter does not access app credentials or private cloud endpoints. Encrypted reasoning is not available; only plaintext reasoning or summaries actually present in the source can be shown. Image and other non-text attachments in exports appear as placeholders.

## Install and launch

Install the prebuilt release with Homebrew on macOS or Linux:

```sh
brew install buengenio/tap/agentbanter
agentbanter open
# http://127.0.0.1:7331
```

Or download an archive from [GitHub Releases](https://github.com/BuenGenio/AgentBanter/releases/latest), extract it, and run `./install.sh`. Source installation is also available with Rust 1.88 or newer: `cargo install --path . --locked`.

`open` starts the server if needed, waits for readiness, and opens your browser. Repeated launches reuse it. Use `agentbanter open --no-browser` for headless use, or `agentbanter serve` to run in the foreground. The default bind address is `127.0.0.1`.

On Linux, add the application to GNOME/KDE with `agentbanter desktop install`. Optional background startup is available through a systemd user service; Homebrew packages provide `brew services` support.

```sh
agentbanter config init         # Create persistent settings; preserves existing files
agentbanter desktop install     # Linux application menu launcher
agentbanter service install     # Linux systemd user unit (does not start it)
agentbanter service start
```

Prebuilt archives are available for macOS and Linux on ARM64 and x86-64, with SHA-256 checksums and a [Homebrew tap](https://github.com/BuenGenio/homebrew-tap). See [packaging, configuration, and release instructions](docs/packaging.md) for configuration, services, and release maintenance.

For development with live asset updates:

```sh
cargo run --release -- serve --templates ./templates --static ./static --open
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

Desktop and CLI coding sessions with local rollout files are discovered recursively, including the year/month/day directory layout. Prompts, replies, plaintext reasoning summaries, function/custom tool calls, and outputs are translated into the shared timeline. Mirrored message and tool events are ignored to avoid duplicates. Recorded turn lifecycle and usage events populate an expandable **Response activity** panel under each prompt: time to first token, cumulative assistant message time, total turn time, and input, cached input, output, and reasoning token counts when available. Metrics update as log records arrive; this does not capture per-token streaming. Timings may overlap, and the log does not split network, queue, and model processing time. A remote session needs its rollout files available locally, for example through a mounted directory:

```sh
agentbanter --sources codex --codex-dir /mounted/codex/sessions
```

Codex and ChatGPT session URLs are prefixed with their source so their IDs cannot collide with Claude IDs. With `--allow-send`, the **Join the conversation** panel lets you select a recorded member of the current agent team. Codex uses `codex queue --thread` and reports queue acceptance separately from a response; its CLI must connect to the server that owns that thread. Claude uses resume and forks active/recent conversations. This does not inject messages into running Claude child agents. The team map uses Codex `source.subagent.thread_spawn.parent_thread_id` and Claude `<parent>/subagents/` transcript locations. It includes parents, siblings and descendants, including across working directories; sharing a project does not imply team membership. Child-agent transcripts are visible but direct messaging is disabled: Codex multi-agent v2 rejects direct input, and the Claude adapter has no child delivery channel. The server enforces this restriction as well as the UI. Missing relationships remain unknown. Recent activity is not a live agent status. The map refreshes every ten seconds.

**All reachable team members** freezes and displays the messageable recipients for a submission and tracks each receipt separately. Transcript-only members are excluded. Partial failures retain the draft; retry only the failed recipients to avoid duplicate messages. CLI failures retain the draft; uncertain delivery should be checked before retrying. Override executables with `--codex-bin` and `--claude-bin`.

Process cards offer **View in conversation** when an executable path matches a recorded tool input or output. The link opens and highlights the entry, enables tools, and pauses Follow. Matches are references, not proof of process creation; cards without evidence say so.

## Workbench and traffic inspector

The workbench keeps conversations in a searchable left drawer with the selected stream and decisions tray on the right. Physical-style RGB and task-light fixtures illuminate the canvas. Click the temperature badge on the task lamp to open its 2200–6500 K slider (default 5000 K); the setting persists locally and reduced-motion preferences stop the rainbow animation.

Tool results offer readable output, expandable typed object trees (including nested JSON strings), formatted JSON, and original records. Quoted JSON strings are decoded in bounded layers. Acorn 8.15.0 parses literal command strings without executing code; Python, JavaScript, CSS, and other heredoc bodies are shown separately. Dynamic expressions remain in the original invocation. Highlight.js 11.11.1 and Acorn are bundled locally with their licenses; the dashboard makes no CDN requests.

The right-hand **Processes & ports** tray refreshes every five seconds when diagnostics are enabled. It shows CPU, resident memory, elapsed runtime, executable, working directory, parent PID, listening ports, and TCP/UDP/Unix sockets. Filters select server listeners, established connections, protocols, processes without sockets, and association confidence; search accepts process names, PIDs, ports, and endpoints. Inspect snapshot opens the colored field tree without disrupting it on refresh. Session environment markers and process ancestry establish association; shared working directories are labeled project candidates. Processes belonging to a different known session are excluded. Environment values are discarded after marker comparison and are never returned to the browser. Snapshots do not prove that a particular turn opened a socket, and processes outside the project or short-lived processes may be missed.

To enable the traffic inspector and process tray, run:

```sh
cargo run --release -- --bind 127.0.0.1 --allow-capture
```

Open **Traffic inspector** in the sidebar and start capture. It observes new records across all configured transcript sources, AgentBanter HTTP request metadata, visible TCP/UDP and Unix socket inventory changes, and tcpdump packet summaries when OS permissions permit. Captures begin at current file ends; existing history stays in the conversation views. Stop terminates collection; Pause view leaves collection running. Records support channel/text filtering, field inspection, and JSON export.

Capture requires explicit loopback-only binding. No TLS decryption, arbitrary application payload interception, or Mach/XPC message contents are provided. Socket inventory is sampled every five seconds and may miss short-lived connections. Packet permission errors appear in the inspector. Known credential fields and reasoning content are omitted, but free-form logs can contain sensitive data. A 4,000-record in-memory ring bounds retention; records and long fields are size-limited and marked when truncated. Export saves the records retained in the browser. No captures persist unless exported. The inspector's own polling is excluded from HTTP observations.

Each right-rail instrument collapses independently and remembers its open state per conversation. The Agents roster retains historical participants found in the recorded family, with expandable details and a recipient-specific message button. Recent activity is based on transcript updates, not confirmed runtime status. Missing or deleted transcripts cannot be reconstructed. Child cards offer a parent messaging shortcut.

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
--open                         Alias for the open command without a subcommand
--allow-send                   Enable supported message delivery routes
--claude-bin <PATH>            Claude sending binary (default: claude)
--send-permission-mode <MODE>  Claude sending permission mode (default: auto)
--templates <DIR>              Override built-in templates; hot-reloaded
--static <DIR>                 Override built-in static assets
--dump-assets <DIR>            Extract templates and assets, then exit
```

Routes: `/` lists sessions, `/latest` opens the most recently modified session, `/s/<id>` shows a session, and `/s/<id>/events` streams updates. `/s/<id>/send` exposes Claude sending/status when enabled.

### Listen addresses and sending

By default, AgentBanter listens on `127.0.0.1`. To expose it on another interface, use an explicit `--bind` address. It makes no model API requests for viewing or importing transcripts. Transcripts can contain sensitive text, and everyone with access to the dashboard can read them.

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
node tests/decoder.cjs
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
