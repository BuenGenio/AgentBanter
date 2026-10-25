//! Read-only process and socket attribution for a selected local conversation.
use crate::{App, AppError};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path as FsPath,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Monitor {
    cache: tokio::sync::Mutex<HashMap<String, (Instant, Value)>>,
}
#[derive(Clone, Default, Serialize, Debug)]
struct Socket {
    fd: String,
    transport: String,
    family: String,
    endpoint: String,
    state: String,
    port: Option<u16>,
}
#[derive(Clone, Default, Serialize, Debug)]
struct Process {
    pid: u32,
    ppid: u32,
    name: String,
    executable: String,
    cwd: String,
    cpu_percent: f64,
    memory_kib: u64,
    elapsed: String,
    attribution: String,
    evidence: String,
    sockets: Vec<Socket>,
    conversation_link: Option<ConversationLink>,
}

#[derive(Clone, Serialize, Debug)]
struct ConversationLink {
    anchor: String,
    evidence: String,
}

// Match complete paths, never a bare program name, port or terminal session ID.
fn mentions_executable(text: &str, process: &Process) -> bool {
    let executable = process
        .executable
        .strip_prefix("./")
        .unwrap_or(&process.executable);
    let relative = FsPath::new(executable)
        .strip_prefix(&process.cwd)
        .ok()
        .and_then(|p| p.to_str());
    [Some(executable), relative]
        .into_iter()
        .flatten()
        .filter(|p| p.contains('/') && *p != "/")
        .any(|path| {
            text.match_indices(path).any(|(start, matched)| {
                let boundary = |c: char| {
                    c.is_whitespace()
                        || matches!(
                            c,
                            '"' | '\''
                                | '`'
                                | ';'
                                | '('
                                | ')'
                                | '{'
                                | '}'
                                | '['
                                | ']'
                                | ','
                                | '='
                                | '\\'
                        )
                };
                (start == 0
                    || text[..start].chars().next_back().is_some_and(boundary)
                    || (text[..start].ends_with("./")
                        && (start == 2
                            || text[..start - 2].chars().next_back().is_some_and(boundary))))
                    && (start + matched.len() == text.len()
                        || text[start + matched.len()..]
                            .chars()
                            .next()
                            .is_some_and(boundary))
            })
        })
}

fn conversation_link(
    process: &Process,
    events: &[crate::model::Event],
) -> Option<ConversationLink> {
    use crate::model::Event;
    let tool_ids: HashSet<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Tool(tool) => Some(tool.id.as_str()),
            _ => None,
        })
        .collect();
    // Prefer input references; results can contain inventories unrelated to launches.
    for output in [false, true] {
        for event in events {
            let (id, text) = match event {
                Event::Tool(tool) if !output => (&tool.id, &tool.detail),
                Event::ToolResult(result)
                    if output && tool_ids.contains(result.tool_id.as_str()) =>
                {
                    (&result.tool_id, &result.detail)
                }
                _ => continue,
            };
            if mentions_executable(&text.replace("\\n", "\n"), process) {
                return Some(ConversationLink {
                    anchor: format!("tool-{id}"),
                    evidence: format!(
                        "Executable path mentioned in tool {}. This is a reference, not proof of launch.",
                        if output { "output" } else { "input" }
                    ),
                });
            }
        }
    }
    None
}

async fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(4), cmd.output()).await {
        Ok(Ok(output)) if output.status.success() || !output.stdout.is_empty() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(Ok(_)) => Err(format!("{program} could not read process information")),
        Ok(Err(_)) => Err(format!("{program} is unavailable")),
        Err(_) => Err(format!("{program} timed out")),
    }
}
fn processes(text: &str) -> HashMap<u32, Process> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|s| s.parse().ok()) else {
            continue;
        };
        let ppid = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let cpu_percent = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let memory_kib = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let elapsed = fields.next().unwrap_or("").to_owned();
        let executable = fields.collect::<Vec<_>>().join(" ");
        let name = FsPath::new(&executable)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if matches!(name.as_str(), "lsof" | "ps") {
            continue;
        }
        out.insert(
            pid,
            Process {
                pid,
                ppid,
                cpu_percent,
                memory_kib,
                elapsed,
                executable,
                name,
                ..Default::default()
            },
        );
    }
    out
}
fn sockets(text: &str, processes: &mut HashMap<u32, Process>) {
    let mut pid = 0;
    let mut socket = Socket::default();
    fn flush(pid: u32, socket: &mut Socket, processes: &mut HashMap<u32, Process>) {
        if let Some(process) = processes.get_mut(&pid) {
            if socket.fd == "cwd" {
                process.cwd = socket.endpoint.clone();
            } else if matches!(socket.family.as_str(), "IPv4" | "IPv6" | "unix") {
                if socket.family == "unix" {
                    socket.transport = "IPC".into();
                }
                socket.port = if socket.transport == "IPC" {
                    None
                } else {
                    socket
                        .endpoint
                        .split("->")
                        .next()
                        .and_then(|s| s.rsplit(':').next())
                        .and_then(|s| s.parse().ok())
                };
                process.sockets.push(socket.clone());
            }
        }
        *socket = Socket::default();
    }
    for line in text.lines().filter(|s| !s.is_empty()) {
        let (key, value) = line.split_at(1);
        match key {
            "p" => {
                flush(pid, &mut socket, processes);
                pid = value.parse().unwrap_or(0);
            }
            "f" => {
                flush(pid, &mut socket, processes);
                socket.fd = value.into();
            }
            "t" => socket.family = value.into(),
            "n" => socket.endpoint = value.into(),
            "P" => socket.transport = value.into(),
            "T" if value.starts_with("ST=") => socket.state = value[3..].into(),
            _ => {}
        }
    }
    flush(pid, &mut socket, processes);
}
fn same_project(cwd: &str, project: &str) -> bool {
    !project.is_empty()
        && project != "/"
        && !cwd.is_empty()
        && FsPath::new(cwd).starts_with(project)
}
fn markers(text: &str, native_id: &str) -> (HashSet<u32>, HashSet<u32>) {
    let (mut current, mut other) = (HashSet::new(), HashSet::new());
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|s| s.parse().ok()) else {
            continue;
        };
        for field in fields {
            if let Some((key, value)) = field.split_once('=')
                && matches!(
                    key,
                    "CODEX_THREAD_ID" | "CODEX_SESSION_ID" | "CLAUDE_SESSION_ID"
                )
            {
                if value == native_id {
                    current.insert(pid);
                } else {
                    other.insert(pid);
                }
            }
        }
    }
    (current, other)
}
fn attribute(
    mut processes: HashMap<u32, Process>,
    project: &str,
    confirmed: &HashSet<u32>,
    other: &HashSet<u32>,
) -> Vec<Process> {
    let mut linked = confirmed.clone();
    loop {
        let before = linked.len();
        for p in processes.values() {
            if linked.contains(&p.ppid) && !other.contains(&p.pid) {
                linked.insert(p.pid);
            }
        }
        if before == linked.len() {
            break;
        }
    }
    let mut out = Vec::new();
    for (_, mut p) in processes.drain() {
        if other.contains(&p.pid) && !confirmed.contains(&p.pid) {
            continue;
        }
        if confirmed.contains(&p.pid) {
            p.attribution = "session".into();
            p.evidence="Inherited environment identifies this conversation. This links the process to the session; it does not prove when each socket opened.".into();
        } else if linked.contains(&p.pid) {
            p.attribution = "descendant".into();
            p.evidence = "Descendant of a process linked to this conversation.".into();
        } else if same_project(&p.cwd, project) {
            p.attribution = "project".into();
            p.evidence =
                "Working directory is inside this project. Session ownership is unconfirmed."
                    .into();
        } else {
            continue;
        }
        out.push(p);
    }
    out.sort_by_key(|p| {
        (
            p.attribution == "project",
            !p.sockets.iter().any(|s| s.state == "LISTEN"),
            p.pid,
        )
    });
    out
}

pub async fn inspect(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    if !app.capture.enabled {
        return Ok(Json(
            json!({"enabled":false,"processes":[],"warnings":["Process monitoring requires --bind 127.0.0.1 --allow-capture."]}),
        ));
    }
    let loaded = app.store.load(&id).await?;
    let project = loaded
        .session
        .read()
        .unwrap()
        .cwd
        .clone()
        .unwrap_or_default();
    if loaded.source == crate::source::Source::Chatgpt {
        return Ok(Json(
            json!({"enabled":true,"processes":[],"warnings":["Imported conversations have no live local process association."]}),
        ));
    }
    let mut cache = app.monitor.cache.lock().await;
    if let Some((at, data)) = cache.get(&id)
        && at.elapsed() < Duration::from_secs(4)
    {
        return Ok(Json(data.clone()));
    }
    let (ps, files) = tokio::join!(
        command("ps", &["-axo", "pid=,ppid=,pcpu=,rss=,etime=,comm="]),
        command("lsof", &["-nP", "-i", "-U", "-d", "cwd", "-FpcftnPT"])
    );
    let mut warnings = Vec::new();
    let mut process_map = processes(&ps.unwrap_or_else(|e| {
        warnings.push(e);
        String::new()
    }));
    sockets(
        &files.unwrap_or_else(|e| {
            warnings.push(e);
            String::new()
        }),
        &mut process_map,
    );
    let candidates: Vec<_> = process_map
        .values()
        .filter(|p| same_project(&p.cwd, &project))
        .map(|p| p.pid.to_string())
        .collect();
    let native = id.strip_prefix("codex--").unwrap_or(&id);
    let (confirmed, other) = if candidates.is_empty() {
        (HashSet::new(), HashSet::new())
    } else {
        // Environment text is used only to compare session markers, then discarded.
        // No environment values or full argv are returned to the browser or capture log.
        match command(
            "ps",
            &["eww", "-p", &candidates.join(","), "-o", "pid=,command="],
        )
        .await
        {
            Ok(text) => markers(&text, native),
            Err(e) => {
                warnings.push(e);
                (HashSet::new(), HashSet::new())
            }
        }
    };
    let mut processes = attribute(process_map, &project, &confirmed, &other);
    {
        let session = loaded.session.read().unwrap();
        for process in &mut processes {
            process.conversation_link = conversation_link(process, &session.events);
        }
    }
    let result = json!({"enabled":true,"observed_at":chrono::Utc::now().to_rfc3339(),"interval_seconds":5,"project":project,"processes":processes,"warnings":warnings,"note":"Live snapshot. Session markers and ancestry establish association; project matches are candidates. Processes that moved outside the project or lost their session marker can be missed. Short-lived processes may exit between samples. Ports are current observations, not a history of socket creation."});
    if cache.len() > 32 {
        cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
    }
    cache.insert(id, (Instant::now(), result.clone()));
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn references_match_paths_not_prefixes_or_session_ids() {
        let process = Process {
            executable: "/project/target/server".into(),
            cwd: "/project".into(),
            pid: 42,
            ..Default::default()
        };
        assert!(mentions_executable(
            "run target/server --port 7331",
            &process
        ));
        assert!(mentions_executable("\"/project/target/server\"", &process));
        assert!(!mentions_executable("target/server-backup", &process));
        assert!(!mentions_executable("/other/target/server", &process));
        assert!(!mentions_executable("session_id: 42, server", &process));
    }
    #[test]
    fn reference_uses_existing_tool_anchor() {
        use crate::model::{Event, ResultView, ToolView};
        let p = Process {
            executable: "/project/server".into(),
            ..Default::default()
        };
        let tool = Event::Tool(ToolView {
            id: "call-123".into(),
            detail: "/project/server --port 7331".into(),
            step_id: String::new(),
            ts: String::new(),
            t: String::new(),
            name: "exec".into(),
            icon: String::new(),
            summary: String::new(),
            files: vec![],
            result: None,
        });
        assert_eq!(
            conversation_link(&p, &[tool]).unwrap().anchor,
            "tool-call-123"
        );
        let orphan = Event::ToolResult(ResultView {
            tool_id: "missing".into(),
            raw: String::new(),
            detail: "/project/server".into(),
            t: String::new(),
            ok: true,
            summary: String::new(),
        });
        assert!(conversation_link(&p, &[orphan]).is_none());
    }
    #[test]
    fn parse_ports_and_cwd() {
        let mut p = processes("42 1 1.5 2048 02:13 /some path/server\n");
        sockets(
            "p42\nfcwd\ntDIR\nn/project\nf7\ntIPv6\nPTCP\nn[::1]:7331\nTST=LISTEN\nf8\ntunix\nn/tmp/test.sock\n",
            &mut p,
        );
        assert_eq!(p[&42].cwd, "/project");
        assert_eq!(p[&42].sockets[0].port, Some(7331));
        assert_eq!(p[&42].sockets[0].state, "LISTEN");
        assert_eq!(p[&42].sockets[1].transport, "IPC");
    }
    #[test]
    fn attribution_does_not_claim_another_session_or_sibling_project() {
        let mut p = processes(
            "42 1 0 10 00:01 server\n43 42 0 10 00:01 worker\n44 1 0 10 00:01 other\n45 1 0 10 00:01 neighbor\n46 1 0 10 00:01 candidate\n",
        );
        for entry in p.values_mut() {
            entry.cwd = "/project".into();
        }
        p.get_mut(&45).unwrap().cwd = "/project-other".into();
        let (current, other) = markers(
            "42 server CODEX_THREAD_ID=one SECRET=never-return\n44 other CODEX_THREAD_ID=two",
            "one",
        );
        let out = attribute(p, "/project", &current, &other);
        assert_eq!(out.len(), 3);
        assert!(
            out.iter()
                .any(|p| p.pid == 43 && p.attribution == "descendant")
        );
        assert!(
            out.iter()
                .any(|p| p.pid == 46 && p.attribution == "project")
        );
        assert!(
            !serde_json::to_string(&out)
                .unwrap()
                .contains("never-return")
        );
    }
}
