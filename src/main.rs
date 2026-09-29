mod assets;
mod model;
mod render;
mod store;
mod text;
mod tools;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use clap::Parser;
use minijinja::context;
use model::Event;
use render::Renderer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use store::{Loaded, SessionMeta, Store};
use tokio_stream::Stream;
use tower_http::services::ServeDir;

/// Live, readable view of Claude Code sessions: prompts, reasoning, tool steps and decisions.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Port to listen on
    #[arg(long, default_value_t = 7331)]
    port: u16,
    /// Address to listen on; repeatable (default: 127.0.0.1 plus this machine's Tailscale IPv4, if any)
    #[arg(long, value_name = "IP")]
    bind: Vec<IpAddr>,
    /// Transcript root (default: ~/.claude/projects)
    #[arg(long)]
    projects_dir: Option<PathBuf>,
    /// Template directory (default: ./templates, else the source tree's)
    #[arg(long)]
    templates: Option<PathBuf>,
    /// Static asset directory (default: ./static, else the source tree's)
    #[arg(long, name = "static")]
    static_dir: Option<PathBuf>,
    /// How often to check transcripts for new lines, in ms
    #[arg(long, default_value_t = 750)]
    poll_ms: u64,
    /// Open the most recent session in the browser
    #[arg(long)]
    open: bool,
    /// Write the built-in templates/ and static/ into DIR (to customise them) and exit
    #[arg(long, value_name = "DIR")]
    dump_assets: Option<PathBuf>,
}

struct App {
    store: Arc<Store>,
    render: Renderer,
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

/// An explicit flag, else `./<name>`, else the source tree's copy; `None` means use the embedded files.
fn resolve_dir(flag: Option<PathBuf>, name: &str) -> anyhow::Result<Option<PathBuf>> {
    if let Some(p) = flag {
        anyhow::ensure!(p.is_dir(), "{} is not a directory", p.display());
        return Ok(Some(p));
    }
    Ok([
        PathBuf::from(name),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name),
    ]
    .into_iter()
    .find(|p| p.is_dir()))
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
        "\nRun with: claude-progress --templates {0}/templates --static {0}/static",
        dir.display()
    );
    Ok(())
}

/// This machine's Tailscale IPv4 address, if `tailscale` is installed and up.
fn tailscale_ip() -> Option<IpAddr> {
    let out = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()?.lines().next()?.trim().parse().ok()
}

async fn embedded_static(Path(path): Path<String>) -> Response {
    match assets::static_file(&path) {
        Some((ct, body)) => ([(axum::http::header::CONTENT_TYPE, ct)], body).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if let Some(dir) = args.dump_assets {
        return dump_assets(&dir);
    }
    let projects_dir = args
        .projects_dir
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude").join("projects")))
        .ok_or_else(|| {
            anyhow::anyhow!("cannot determine ~/.claude/projects; pass --projects-dir")
        })?;
    let templates = resolve_dir(args.templates, "templates")?;
    let static_dir = resolve_dir(args.static_dir, "static")?;

    let store = Store::new(projects_dir.clone());
    store.spawn_tailer(Duration::from_millis(args.poll_ms));
    let app = Arc::new(App {
        store,
        render: Renderer::new(templates.clone()),
    });

    let router = Router::new()
        .route("/", get(index))
        .route("/latest", get(latest))
        .route("/s/{id}", get(session))
        .route("/s/{id}/events", get(events));
    let router = match &static_dir {
        Some(dir) => router.nest_service("/static", ServeDir::new(dir)),
        None => router.route("/static/{*path}", get(embedded_static)),
    };
    let router = router.with_state(app);

    let ips = if args.bind.is_empty() {
        let mut ips = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        match tailscale_ip() {
            Some(ip) => ips.push(ip),
            None => eprintln!("tailscale: no IPv4 address found, listening on 127.0.0.1 only"),
        }
        ips
    } else {
        args.bind
    };
    let mut listeners = Vec::new();
    for ip in ips {
        let addr = SocketAddr::new(ip, args.port);
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
        listeners.push((addr, listener));
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
        "claude-progress  {}\n  transcripts: {}\n  templates:   {}\n  static:      {}",
        urls.join("  "),
        projects_dir.display(),
        shown(&templates),
        shown(&static_dir)
    );
    if args.open {
        let _ = std::process::Command::new("xdg-open")
            .arg(format!("{}/latest", urls[0]))
            .spawn();
    }
    let mut servers = tokio::task::JoinSet::new();
    for (_, listener) in listeners {
        servers.spawn(axum::serve(listener, router.clone()).into_future());
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
}

async fn index(State(app): State<Arc<App>>) -> Result<Html<String>, AppError> {
    let store = app.store.clone();
    let metas = tokio::task::spawn_blocking(move || store.scan()).await?;
    let total = metas.len();
    let mut groups: Vec<ProjectGroup> = Vec::new();
    for m in metas {
        match groups.iter_mut().find(|g| g.cwd == m.cwd) {
            Some(g) => g.sessions.push(m),
            None => groups.push(ProjectGroup {
                cwd: m.cwd.clone(),
                project: m.project.clone(),
                sessions: vec![m],
            }),
        }
    }
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
    let s = loaded.session.read().unwrap();
    let html = app.render.render(
        "session.html",
        context! {
            s => s.view(),
            stats => s.session_stats(),
            blocks => s.blocks(),
            decisions => s.decisions(),
            seq => s.events.len(),
        },
    )?;
    Ok(Html(html))
}

#[derive(Deserialize)]
struct FromQuery {
    from: Option<usize>,
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
        loop {
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
            Event::Tool(t) => {
                json!({ "kind": "tool", "step": t.step_id, "html": r("partials/tool.html", context! { tool => t }) })
            }
            Event::ToolResult(rv) => json!({
                "kind": "tool_result", "tool": rv.tool_id, "ok": rv.ok, "detail": rv.detail,
                "html": r("partials/result.html", context! { r => rv }),
            }),
            Event::Decision(d) => json!({
                "kind": "decision",
                "html": r("partials/block.html", context! { b => model::Block::Decision(d.clone()) }),
                "panel": r("partials/decision_panel.html", context! { d => d }),
            }),
            _ => {
                let Some(b) = s.block_at(i) else { continue };
                json!({ "kind": "block", "html": r("partials/block.html", context! { b => b }) })
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
