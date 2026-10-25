mod assets;
mod capture;
mod chatgpt;
mod install;
mod launch;
mod model;
mod monitor;
mod ollama;
mod render;
mod search;
mod send;
mod source;
mod store;
mod team;
mod text;
mod tools;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use minijinja::context;
use model::Event;
use render::Renderer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use source::Source;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use store::{Loaded, SessionMeta, Store};
use tokio_stream::Stream;
use tower_http::services::ServeDir;

/// AgentBanter: follow Codex, ChatGPT and Claude Code conversations.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Action>,
    /// Configuration file (default: OS config directory/agentbanter/config.json)
    #[arg(long, global = true, value_name = "JSON")]
    config: Option<PathBuf>,
    /// Print the dashboard URL without launching a browser
    #[arg(long, global = true)]
    no_browser: bool,
    /// Port to listen on
    #[arg(long, global = true, default_value_t = 7331)]
    port: u16,
    /// Enable local diagnostics capture (requires explicit loopback-only --bind).
    #[arg(long, global = true)]
    allow_capture: bool,
    /// Address to listen on; repeatable (default: 127.0.0.1)
    #[arg(long, global = true, value_name = "IP")]
    bind: Vec<IpAddr>,
    /// Transcript root (default: ~/.claude/projects)
    #[arg(long, global = true)]
    projects_dir: Option<PathBuf>,
    /// Sources to display (comma-separated)
    #[arg(
        long,
        global = true,
        value_enum,
        value_delimiter = ',',
        default_value = "claude,codex,chatgpt,ollama"
    )]
    sources: Vec<Source>,
    /// Codex rollout root (default: $CODEX_HOME/sessions or ~/.codex/sessions)
    #[arg(long, global = true)]
    codex_dir: Option<PathBuf>,
    /// Imported ChatGPT transcripts (default: OS data directory/agentbanter/chatgpt)
    #[arg(long, global = true)]
    chatgpt_dir: Option<PathBuf>,
    /// Enable local Ollama chat and the recording proxy (loopback binding required)
    #[arg(long, global = true)]
    allow_ollama: bool,
    /// Local Ollama HTTP endpoint
    #[arg(long, global = true, default_value = "http://127.0.0.1:11434")]
    ollama_url: String,
    /// Recorded Ollama conversations (default: OS data directory/agentbanter/ollama)
    #[arg(long, global = true)]
    ollama_dir: Option<PathBuf>,
    /// Import an unzipped ChatGPT conversations.json into --chatgpt-dir, then exit
    #[arg(long, global = true, value_name = "JSON")]
    import_chatgpt: Option<PathBuf>,
    /// Override embedded templates (hot reload)
    #[arg(long, global = true)]
    templates: Option<PathBuf>,
    /// Override embedded static assets
    #[arg(long = "static", global = true)]
    static_dir: Option<PathBuf>,
    /// How often to check transcripts for new lines, in ms
    #[arg(long, global = true, default_value_t = 750)]
    poll_ms: u64,
    /// Open the dashboard (alias for `open` without a subcommand)
    #[arg(long, global = true)]
    open: bool,
    /// Show a message box on session pages that continues the session via `claude -p --resume`.
    /// Anyone who can open the page can then run Claude as you, so it is off by default.
    #[arg(long, global = true)]
    allow_send: bool,
    /// Codex CLI used to queue messages to existing threads
    #[arg(long, global = true, value_name = "PATH", default_value = "codex")]
    codex_bin: PathBuf,
    /// Claude Code binary used for sending
    #[arg(long, global = true, value_name = "PATH", default_value = "claude")]
    claude_bin: PathBuf,
    /// Permission mode for sent messages (headless runs cannot ask; see `claude --help`)
    #[arg(long, global = true, value_name = "MODE", default_value = "auto")]
    send_permission_mode: String,
    /// Write the built-in templates/ and static/ into DIR (to customise them) and exit
    #[arg(long, global = true, value_name = "DIR")]
    dump_assets: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Action {
    /// Start or reuse the local server and open its dashboard
    Open,
    /// Run the server in the foreground (also the default without a command)
    Serve,
    /// Install or remove the GNOME/KDE application launcher
    Desktop {
        #[command(subcommand)]
        action: InstallAction,
    },
    /// Manage a user-level systemd service on Linux
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Create a persistent configuration file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}
#[derive(Subcommand, Clone, Copy)]
enum InstallAction {
    Install,
    Uninstall,
}
#[derive(Subcommand, Clone, Copy)]
enum ServiceAction {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
}
#[derive(Subcommand)]
enum ConfigAction {
    Init,
}

struct App {
    store: Arc<Store>,
    capture: Arc<capture::Capture>,
    monitor: monitor::Monitor,
    search: Arc<search::Search>,
    ollama: Arc<ollama::Ollama>,
    render: Renderer,
    sender: Option<Arc<send::Sender>>,
}

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("error: {:#}", self.0),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

/// Installed binaries always use embedded assets unless an override is explicit.
fn resolve_dir(flag: Option<PathBuf>, _name: &str) -> anyhow::Result<Option<PathBuf>> {
    if let Some(p) = &flag {
        anyhow::ensure!(p.is_dir(), "{} is not a directory", p.display());
    }
    Ok(flag)
}

fn dump_assets(dir: &std::path::Path) -> anyhow::Result<()> {
    let files = assets::TEMPLATES
        .iter()
        .map(|(n, body)| (PathBuf::from("templates").join(n), *body))
        .chain(
            assets::STATIC
                .iter()
                .map(|(n, _, body)| (PathBuf::from("static").join(n), *body)),
        );
    for (rel, body) in files {
        let path = dir.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, body)?;
        println!("wrote {}", path.display());
    }
    println!(
        "\nRun with: agentbanter --templates {0}/templates --static {0}/static",
        dir.display()
    );
    Ok(())
}

async fn embedded_static(Path(path): Path<String>) -> Response {
    match assets::static_file(&path) {
        Some((ct, body)) => ([(axum::http::header::CONTENT_TYPE, ct)], body).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Args::command().get_matches();
    let mut args = Args::from_arg_matches(&matches)?;
    if matches!(
        args.command,
        Some(Action::Config {
            action: ConfigAction::Init
        })
    ) {
        return launch::init_config(args.config.as_deref());
    }
    launch::configure(&mut args, &matches)?;
    anyhow::ensure!(args.poll_ms >= 50, "--poll-ms must be at least 50");
    anyhow::ensure!(!args.sources.is_empty(), "at least one source is required");
    if args.command.is_none() && args.open {
        return launch::open(&args).await;
    }
    match &args.command {
        Some(Action::Open) => return launch::open(&args).await,
        Some(Action::Desktop { action }) => return install::desktop(*action),
        Some(Action::Service { action }) => {
            return install::service(*action, args.config.as_deref());
        }
        Some(Action::Config {
            action: ConfigAction::Init,
        }) => return launch::init_config(args.config.as_deref()),
        _ => {}
    }
    if let Some(dir) = args.dump_assets {
        return dump_assets(&dir);
    }
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let chatgpt_dir = args.chatgpt_dir.unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(|| home.join(".local/share"))
            .join("agentbanter/chatgpt")
    });
    if let Some(path) = args.import_chatgpt {
        let count = chatgpt::import(&path, &chatgpt_dir)?;
        println!(
            "Imported {count} ChatGPT conversations into {}",
            chatgpt_dir.display()
        );
        return Ok(());
    }
    let projects_dir = args
        .projects_dir
        .unwrap_or_else(|| home.join(".claude/projects"));
    let codex_dir = args.codex_dir.unwrap_or_else(|| {
        std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"))
            .join("sessions")
    });
    let ollama_dir = args.ollama_dir.clone().unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(|| home.join(".local/share"))
            .join("agentbanter/ollama")
    });
    anyhow::ensure!(
        !args.allow_ollama || args.bind.iter().all(IpAddr::is_loopback),
        "--allow-ollama requires loopback-only binding"
    );
    let local_models =
        ollama::Ollama::new(&args.ollama_url, ollama_dir.clone(), args.allow_ollama)?;
    let roots: Vec<_> = [
        (Source::Claude, projects_dir),
        (Source::Codex, codex_dir),
        (Source::Chatgpt, chatgpt_dir),
        (Source::Ollama, ollama_dir),
    ]
    .into_iter()
    .filter(|(source, _)| {
        args.sources.contains(source) || (*source == Source::Ollama && args.allow_ollama)
    })
    .collect();
    let shown_roots = roots
        .iter()
        .map(|(s, p)| format!("{}: {}", s.label(), p.display()))
        .collect::<Vec<_>>()
        .join("\n  ");
    let templates = resolve_dir(args.templates, "templates")?;
    let static_dir = resolve_dir(args.static_dir, "static")?;

    let store = Store::new(roots);
    store.spawn_tailer(Duration::from_millis(args.poll_ms));
    anyhow::ensure!(
        !args.allow_capture || (!args.bind.is_empty() && args.bind.iter().all(IpAddr::is_loopback)),
        "--allow-capture requires explicit loopback-only --bind 127.0.0.1"
    );
    let app = Arc::new(App {
        capture: capture::Capture::new(args.allow_capture),
        monitor: monitor::Monitor::default(),
        search: Arc::new(search::Search::default()),
        ollama: local_models,
        store,
        render: Renderer::new(templates.clone()),
        sender: args
            .allow_send
            .then(|| send::Sender::new(args.claude_bin, args.codex_bin, args.send_permission_mode)),
    });

    let router = Router::new()
        .route("/", get(index))
        .route(
            "/healthz",
            get(|| async {
                Json(json!({"app":"agentbanter", "version":env!("CARGO_PKG_VERSION"), "pid":std::process::id()}))
            }),
        )
        .route("/latest", get(latest))
        .route("/search", get(search::query))
        .route("/local", get(ollama::page))
        .route("/local/models", get(ollama::models))
        .route("/local/model", axum::routing::post(ollama::model_info))
        .route("/local/chat", axum::routing::post(ollama::chat))
        .route("/local/traces", get(ollama::traces))
        .route("/local/traces/{id}", get(ollama::trace))
        .route("/local/traces/{id}/analysis", axum::routing::post(ollama::analyze))
        .route("/ollama/{*endpoint}", get(ollama::proxy_get).post(ollama::proxy_post).layer(axum::extract::DefaultBodyLimit::max(32*1024*1024)))
        .route("/capture", get(capture::page))
        .route("/capture/records", get(capture::records))
        .route("/capture/control", axum::routing::post(capture::control))
        .route("/s/{id}", get(session))
        .route("/s/{id}/events", get(events))
        .route("/s/{id}/processes", get(monitor::inspect))
        .route("/s/{id}/team", get(session_team))
        .route("/s/{id}/send", get(send_status).post(send_message));
    let router = match &static_dir {
        Some(dir) => router.nest_service("/static", ServeDir::new(dir)),
        None => router.route("/static/{*path}", get(embedded_static)),
    };
    let router = router
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            capture::http_trace,
        ))
        .with_state(app);

    let ips = if args.bind.is_empty() {
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
    } else {
        args.bind
    };
    let mut listeners = Vec::new();
    for ip in ips {
        let addr = SocketAddr::new(ip, args.port);
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
        listeners.push((listener.local_addr()?, listener));
    }
    let urls: Vec<String> = listeners
        .iter()
        .map(|(addr, _)| format!("http://{addr}"))
        .collect();
    let shown = |d: &Option<PathBuf>| {
        d.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "embedded".into())
    };
    eprintln!(
        "AgentBanter  {}\n  {}\n  templates:   {}\n  static:      {}",
        urls.join("  "),
        shown_roots,
        shown(&templates),
        shown(&static_dir)
    );
    let mut servers = tokio::task::JoinSet::new();
    for (_, listener) in listeners {
        servers.spawn(axum::serve(listener, router.clone()).into_future());
    }
    if args.open
        && !args.no_browser
        && let Err(error) = launch::browser(&urls[0]).await
    {
        eprintln!("{error:#} · open {} manually", urls[0]);
    }
    // Any listener stopping (only on error) ends the process.
    if let Some(res) = servers.join_next().await {
        res??;
    }
    Ok(())
}

#[derive(Serialize)]
struct ProjectGroup {
    cwd: String,
    project: String,
    sessions: Vec<SessionMeta>,
    tree: Vec<serde_json::Value>,
}

fn group_sessions(metas: Vec<SessionMeta>) -> Vec<ProjectGroup> {
    use std::collections::{HashMap, HashSet};
    fn node(
        meta: &SessionMeta,
        all: &[SessionMeta],
        seen: &mut HashSet<String>,
        depth: usize,
    ) -> serde_json::Value {
        seen.insert(meta.id.clone());
        let mut children = Vec::new();
        if depth < 32 {
            for child in all
                .iter()
                .filter(|m| m.parent_id.as_deref() == Some(meta.id.as_str()))
            {
                if !seen.contains(&child.id) {
                    children.push(node(child, all, seen, depth + 1));
                }
            }
        }
        let mut value = serde_json::to_value(meta).unwrap();
        value["children"] = serde_json::Value::Array(children);
        value
    }
    let mut groups: Vec<ProjectGroup> = Vec::new();
    let mut by_cwd = HashMap::new();
    for m in &metas {
        let index = *by_cwd.entry(m.cwd.clone()).or_insert_with(|| {
            let i = groups.len();
            groups.push(ProjectGroup {
                cwd: m.cwd.clone(),
                project: m.project.clone(),
                sessions: Vec::new(),
                tree: Vec::new(),
            });
            i
        });
        groups[index].sessions.push(m.clone());
    }
    let ids: HashSet<_> = metas.iter().map(|m| m.id.as_str()).collect();
    let mut seen = HashSet::new();
    let roots = metas
        .iter()
        .filter(|m| m.parent_id.as_deref().is_none_or(|p| !ids.contains(p)))
        .chain(metas.iter());
    for m in roots {
        if !seen.contains(&m.id) {
            groups[by_cwd[&m.cwd]]
                .tree
                .push(node(m, &metas, &mut seen, 0));
        }
    }
    groups
}

async fn index(State(app): State<Arc<App>>) -> Result<Html<String>, AppError> {
    let store = app.store.clone();
    let metas = tokio::task::spawn_blocking(move || store.scan()).await?;
    let total = metas.len();
    let groups = group_sessions(metas);
    Ok(Html(app.render.render(
        "index.html",
        context! { projects => groups, total },
    )?))
}

async fn latest(State(app): State<Arc<App>>) -> Result<Redirect, AppError> {
    let store = app.store.clone();
    let metas = tokio::task::spawn_blocking(move || store.scan()).await?;
    let first = metas
        .first()
        .ok_or_else(|| anyhow::anyhow!("no sessions found"))?;
    Ok(Redirect::temporary(&format!("/s/{}", first.id)))
}

async fn session(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Html<String>, AppError> {
    let loaded = app.store.load(&id).await?;
    let store = app.store.clone();
    let metas = tokio::task::spawn_blocking(move || store.scan()).await?;
    let total = metas.len();
    let groups = group_sessions(metas);
    let s = loaded.session.read().unwrap();
    let html = app.render.render(
        "session.html",
        context! {
            projects => groups, total, selected => id,
            s => s.view(),
            stats => s.session_stats(),
            blocks => s.blocks(),
            decisions => s.decisions(),
            seq => s.events.len(),
            can_send => app.sender.is_some() && !matches!(loaded.source,Source::Chatgpt|Source::Ollama),
            is_ollama => loaded.source==Source::Ollama,

            agent => loaded.source.label(),
            is_import => loaded.source == Source::Chatgpt,
            generation => loaded.generation.load(Ordering::SeqCst),
        },
    )?;
    Ok(Html(html))
}

async fn session_team(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let store = app.store.clone();
    let metas = tokio::task::spawn_blocking(move || store.scan()).await?;
    if !metas.iter().any(|m| m.id == id) {
        return Err(anyhow::anyhow!("session not found").into());
    }
    Ok(Json(team::view(&metas, &id)))
}

#[derive(Deserialize)]
struct SendBody {
    text: String,
}

fn send_error(code: StatusCode, msg: impl Into<String>) -> Response {
    (
        code,
        Json(json!({ "state": "failed", "error": msg.into() })),
    )
        .into_response()
}

async fn send_status(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    if id.starts_with("chatgpt--") {
        return send_error(
            StatusCode::FORBIDDEN,
            "imported conversations cannot receive messages",
        );
    }
    match &app.sender {
        Some(sender) => Json(sender.status(&id)).into_response(),
        None => send_error(
            StatusCode::NOT_FOUND,
            "sending is disabled (start with --allow-send)",
        ),
    }
}

/// Continue a session with a new prompt. Forks when the session is open in another process or was
/// written to moments ago by something other than us, so a live chat is never appended to twice.
async fn send_message(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<SendBody>>,
) -> Response {
    if id.starts_with("chatgpt--") {
        return send_error(
            StatusCode::FORBIDDEN,
            "imported conversations cannot receive messages",
        );
    }
    let Some(sender) = &app.sender else {
        return send_error(
            StatusCode::NOT_FOUND,
            "sending is disabled (start with --allow-send)",
        );
    };
    // Cross-site pages can't send a JSON body without a preflight we never answer, and a present
    // Origin must match Host; together these stop other sites from driving Claude through the browser.
    let host = headers.get("host").and_then(|h| h.to_str().ok());
    let origin_ok = match headers.get("origin").and_then(|o| o.to_str().ok()) {
        None => true,
        Some(o) => host.is_some_and(|h| o.split_once("://").is_some_and(|(_, rest)| rest == h)),
    };
    if !origin_ok {
        return send_error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let Some(Json(body)) = body else {
        return send_error(
            StatusCode::BAD_REQUEST,
            "expected a JSON body {\"text\": …}",
        );
    };
    let text = body.text.trim().to_string();
    if text.len() > 32_000 {
        return send_error(StatusCode::BAD_REQUEST, "message exceeds 32,000 bytes");
    }
    if text.is_empty() {
        return send_error(StatusCode::BAD_REQUEST, "empty message");
    }
    let store = app.store.clone();
    let id2 = id.clone();
    let Ok(Some(meta)) =
        tokio::task::spawn_blocking(move || store.scan().into_iter().find(|m| m.id == id2)).await
    else {
        return send_error(StatusCode::NOT_FOUND, "session not found");
    };
    if meta.source == Source::Ollama {
        return send_error(
            StatusCode::BAD_REQUEST,
            "Open Local models to continue this Ollama conversation.",
        );
    }
    if !team::can_message(&meta) {
        return send_error(
            StatusCode::FORBIDDEN,
            "This child transcript cannot receive direct messages. Select its parent conversation.",
        );
    }
    if meta.source == Source::Codex {
        return match sender.queue_codex(&id, PathBuf::from(&meta.cwd), text) {
            Ok(()) => (StatusCode::ACCEPTED, Json(sender.status(&id))).into_response(),
            Err(e) => send_error(StatusCode::CONFLICT, e.to_string()),
        };
    }
    let recent = (chrono::Utc::now() - meta.mtime).num_seconds() < 60;
    let ours = matches!(sender.status(&id), send::SendStatus::Done { .. });
    let fork = send::held_elsewhere(&id) || (recent && !ours);
    match sender.start(&id, PathBuf::from(&meta.cwd), text, fork) {
        Ok(()) => (StatusCode::ACCEPTED, Json(sender.status(&id))).into_response(),
        Err(e) => send_error(StatusCode::CONFLICT, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FromQuery {
    from: Option<usize>,
    generation: Option<u64>,
}

async fn events(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FromQuery>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, AppError> {
    let loaded = app.store.load(&id).await?;
    // On reconnect the browser tells us the last id it saw; that beats the page's initial `from`.
    let from = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| n + 1)
        .or(q.from)
        .unwrap_or(0);
    let mut rx = app.store.tx.subscribe();

    let stream = async_stream::stream! {
        let mut sent = from;
        let generation = q.generation.unwrap_or_else(||loaded.generation.load(Ordering::SeqCst));
        loop {
            if loaded.generation.load(Ordering::SeqCst) != generation {
                yield Ok(SseEvent::default().data(json!({"kind":"reset"}).to_string()));
                break;
            }
            for (seq, payload) in render_since(&app, &loaded, sent) {
                sent = sent.max(seq + 1);
                yield Ok(SseEvent::default().id(seq.to_string()).data(payload));
            }
            match rx.recv().await {
                Ok(u) if u.session_id == id => {}
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Render every event from `from` as an SSE payload, followed by a stats refresh.
fn render_since(app: &App, loaded: &Loaded, from: usize) -> Vec<(usize, String)> {
    let s = loaded.session.read().unwrap();
    let r = |name: &str, ctx: minijinja::Value| {
        app.render
            .render(name, ctx)
            .unwrap_or_else(|e| format!("<pre class=\"tpl-error\">{e}</pre>"))
    };
    let mut out = Vec::new();
    for i in from..s.events.len() {
        let payload = match &s.events[i] {
            Event::Metrics { prompt, .. } => json!({
                "kind": "stats", "prompt": s.prompt_id(*prompt),
                "chips": r("partials/chips.html", context! { stats => s.prompt_stats(*prompt) }),
                "header": r("partials/header_stats.html", context! { stats => s.session_stats() }),
                "dcount": s.decisions().len(),
            }),
            Event::Tool(t) => {
                json!({ "kind": "tool", "step": t.step_id, "html": r("partials/tool.html", context! { tool => t }) })
            }
            Event::ToolResult(rv) => json!({
                "kind": "tool_result", "tool": rv.tool_id, "ok": rv.ok, "detail": rv.detail, "raw": rv.raw,
                "html": r("partials/result.html", context! { r => rv }),
            }),
            Event::Decision(d) => json!({
                "kind": "decision",
                "html": r("partials/block.html", context! { b => model::Block::Decision(d.clone()), agent => loaded.source.label() }),
                "panel": r("partials/decision_panel.html", context! { d => d }),
            }),
            _ => {
                let Some(b) = s.block_at(i) else { continue };
                json!({ "kind": "block", "html": r("partials/block.html", context! { b => b, agent => loaded.source.label() }) })
            }
        };
        out.push((i, payload.to_string()));
    }
    if let Some(&(last, _)) = out.last() {
        let prompt = s.last_prompt_idx();
        let stats = json!({
            "kind": "stats",
            "prompt": prompt.and_then(|p| s.prompt_id(p)),
            "chips": prompt.map(|p| r("partials/chips.html", context! { stats => s.prompt_stats(p) })),
            "header": r("partials/header_stats.html", context! { stats => s.session_stats() }),
            "dcount": s.decisions().len(),
        });
        out.push((last, stats.to_string()));
    }
    out
}

#[cfg(test)]
mod drawer_tests {
    use super::*;
    #[test]
    fn hierarchy_keeps_cross_project_children_once_and_bounds_cycles() {
        let make = |id: &str, parent: Option<&str>, cwd: &str| SessionMeta {
            id: id.into(),
            source: Source::Codex,
            agent: "Codex".into(),
            parent_id: parent.map(str::to_owned),
            agent_name: None,
            agent_task: None,
            path: Default::default(),
            slug: String::new(),
            cwd: cwd.into(),
            project: cwd.into(),
            title: id.into(),
            mtime: chrono::Utc::now(),
            age: String::new(),
            size: String::new(),
            live: false,
        };
        let groups = group_sessions(vec![
            make("child", Some("root"), "/other"),
            make("root", None, "/project"),
            make("a", Some("b"), "/cycle"),
            make("b", Some("a"), "/cycle"),
            make("orphan", Some("missing"), "/project"),
        ]);
        let root = groups.iter().find(|g| g.cwd == "/project").unwrap();
        assert_eq!(root.tree[0]["id"], "root");
        assert_eq!(root.tree[0]["children"][0]["id"], "child");
        assert_eq!(root.tree[1]["id"], "orphan");
        assert!(
            groups
                .iter()
                .find(|g| g.cwd == "/other")
                .unwrap()
                .tree
                .is_empty()
        );
        let cycle = groups.iter().find(|g| g.cwd == "/cycle").unwrap();
        assert_eq!(cycle.tree.len(), 1);
        assert_eq!(cycle.tree[0]["children"].as_array().unwrap().len(), 1);
        assert_eq!(cycle.tree[0]["children"][0]["children"], json!([]));
    }
}
