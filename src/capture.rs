//! Bounded, opt-in local diagnostics. Socket inventories are not message captures.
use crate::{App, AppError};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, BufReader};

const CAPACITY: usize = 4000;
#[derive(Clone, Serialize)]
pub struct Entry {
    pub seq: u64,
    pub time: String,
    pub channel: String,
    pub session: String,
    pub kind: String,
    pub detail: Value,
}
#[derive(Default)]
struct Buffer {
    seq: u64,
    dropped: u64,
    entries: VecDeque<Entry>,
}
pub struct Capture {
    pub enabled: bool,
    running: AtomicBool,
    generation: AtomicU64,
    buffer: Mutex<Buffer>,
}
impl Capture {
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled,
            running: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            buffer: Mutex::new(Buffer::default()),
        })
    }
    fn active(&self, generation: u64) -> bool {
        self.running.load(Ordering::SeqCst) && self.generation.load(Ordering::SeqCst) == generation
    }
    pub fn record(&self, channel: &str, session: &str, kind: &str, detail: Value) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }
        let mut buffer = self.buffer.lock().unwrap();
        buffer.seq += 1;
        let mut detail = sanitize(detail);
        let serialized = detail.to_string();
        if serialized.len() > 32768 {
            detail = json!({"truncated":true,"original_bytes":serialized.len(),"preview":serialized.chars().take(8192).collect::<String>()});
        }
        let entry = Entry {
            seq: buffer.seq,
            time: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            channel: channel.into(),
            session: session.into(),
            kind: kind.into(),
            detail,
        };
        if buffer.entries.len() == CAPACITY {
            buffer.entries.pop_front();
            buffer.dropped += 1;
        }
        buffer.entries.push_back(entry);
    }
    fn start(self: &Arc<Self>, store: Arc<crate::store::Store>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.record("internal", "", "capture.started", json!({"scope":"All configured local transcripts; dashboard HTTP; visible TCP, UDP and Unix sockets; packet summaries if permitted", "limits":"No TLS decryption, arbitrary process memory, Mach/XPC message payloads or encrypted reasoning. Ring buffer retains 4000 records. Transcript records larger than 1 MiB are skipped."}));
        let capture = self.clone();
        tokio::spawn(async move {
            let mut offsets: HashMap<PathBuf, u64> = HashMap::new();
            let mut initial_scan = true;
            while capture.active(generation) {
                let c = capture.clone();
                let s = store.clone();
                offsets = tokio::task::spawn_blocking(move || {
                    for meta in s.scan() {
                        if !c.active(generation) { break; }
                        let Ok(mut file) = std::fs::File::open(&meta.path) else { continue };
                        let len = file.metadata().map(|m|m.len()).unwrap_or(0);
                        let previous = offsets.get(&meta.path).copied();
                        // Start at EOF: this is live capture, not a historical import.
                        let start = previous.filter(|offset| *offset <= len).unwrap_or(if initial_scan { len } else { 0 });
                        if file.seek(SeekFrom::Start(start)).is_err() { continue; }
                        let mut bytes = Vec::new();
                        if file.take(1024 * 1024).read_to_end(&mut bytes).is_err() { continue; }
                        if let Some(last) = bytes.iter().rposition(|b| *b == b'\n') {
                            for line in bytes[..=last].split(|b| *b == b'\n').filter(|b| !b.is_empty()) {
                                match serde_json::from_slice::<Value>(line) {
                                    Ok(record) => {
                                        let kind = format!("{} / {}", record["type"].as_str().unwrap_or("record"), record["payload"]["type"].as_str().unwrap_or("data"));
                                        c.record("transcript", &meta.id, &kind, record);
                                    }
                                    Err(_) => c.record("transcript", &meta.id, "parse.error", json!({"bytes":line.len()})),
                                }
                            }
                            offsets.insert(meta.path, start + last as u64 + 1);
                        } else if bytes.len() == 1024 * 1024 {
                            c.record("transcript", &meta.id, "record.skipped", json!({"reason":"Record exceeds 1 MiB; advancing to current EOF", "bytes":len-start}));
                            offsets.insert(meta.path, len);
                        } else { offsets.entry(meta.path).or_insert(start); }
                    }
                    offsets
                }).await.unwrap_or_default();
                initial_scan = false;
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        });
        let capture = self.clone();
        tokio::spawn(async move {
            let mut previous = HashSet::new();
            while capture.active(generation) {
                let mut command = tokio::process::Command::new("lsof");
                command
                    .args(["-nP", "-i", "-U", "-Fpcftn"])
                    .kill_on_drop(true);
                match tokio::time::timeout(Duration::from_secs(4), command.output()).await {
                    Ok(Ok(output)) => {
                        let sockets = parse_sockets(&String::from_utf8_lossy(&output.stdout));
                        let current: HashSet<String> =
                            sockets.iter().map(Value::to_string).collect();
                        if !capture.active(generation) {
                            break;
                        }
                        for socket in &sockets {
                            if !previous.contains(&socket.to_string()) {
                                capture.record(
                                    if socket["type"] == "unix" {
                                        "ipc"
                                    } else {
                                        "network"
                                    },
                                    "",
                                    "socket.observed",
                                    socket.clone(),
                                );
                            }
                        }
                        for closed in previous.difference(&current) {
                            if let Ok(socket) = serde_json::from_str::<Value>(closed) {
                                capture.record(
                                    if socket["type"] == "unix" {
                                        "ipc"
                                    } else {
                                        "network"
                                    },
                                    "",
                                    "socket.no_longer_visible",
                                    socket,
                                );
                            }
                        }
                        if sockets.is_empty() && !output.status.success() {
                            capture.record(
                                "internal",
                                "",
                                "socket.inventory_unavailable",
                                json!({"error":String::from_utf8_lossy(&output.stderr)}),
                            );
                        }
                        previous = current;
                    }
                    _ => capture.record(
                        "internal",
                        "",
                        "socket.inventory_unavailable",
                        json!({"reason":"lsof unavailable or timed out"}),
                    ),
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        let capture = self.clone();
        tokio::spawn(async move {
            let interface = if cfg!(target_os = "macos") {
                "pktap,all"
            } else {
                "any"
            };
            let child = tokio::process::Command::new("tcpdump")
                .args(["-i", interface, "-n", "-q", "-tttt", "-l", "-s", "96"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn();
            let Ok(mut child) = child else {
                capture.record(
                    "packet",
                    "",
                    "capture.unavailable",
                    json!({"reason":"tcpdump is not installed or executable"}),
                );
                return;
            };
            let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
            let mut errors = BufReader::new(child.stderr.take().unwrap()).lines();
            loop {
                if !capture.active(generation) {
                    let _ = child.kill().await;
                    break;
                }
                tokio::select! {
                    line = output.next_line() => match line {
                        Ok(Some(line)) => capture.record("packet", "", "packet.summary", json!({"summary":line})),
                        _ => {
                            while let Ok(Some(error)) = errors.next_line().await { capture.record("packet", "", "capture.status", json!({"message":error})); }
                            let _ = child.wait().await;
                            break;
                        }
                    },
                    _ = tokio::time::sleep(Duration::from_millis(300)) => {}
                }
            }
        });
    }
}

fn parse_sockets(text: &str) -> Vec<Value> {
    let (mut pid, mut process, mut fd, mut typ) =
        (String::new(), String::new(), String::new(), String::new());
    let mut sockets = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (key, value) = line.split_at(1);
        match key {
            "p" => { pid = value.into(); process.clear(); fd.clear(); typ.clear(); }
            "c" => process = value.into(), "f" => { fd = value.into(); typ.clear(); }, "t" => typ = value.into(),
            "n" => sockets.push(json!({"pid":pid,"process":process,"fd":fd,"type":typ,"endpoint":value,"observation":"Socket inventory; no message payload"})),
            _ => {}
        }
    }
    sockets
}

fn sanitize(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        if object.get("type").and_then(Value::as_str) == Some("reasoning")
            || object.get("channel").and_then(Value::as_str) == Some("analysis")
        {
            return json!({"type":object.get("type"),"channel":object.get("channel"),"content":"[Reasoning content omitted from traffic capture]"});
        }
        if matches!(
            object.get("role").and_then(Value::as_str),
            Some("system" | "developer")
        ) {
            return json!({"role":object.get("role"),"content":"[Instruction content omitted]"});
        }
        for (key, value) in object.iter_mut() {
            if [
                "encrypted_content",
                "authorization",
                "cookie",
                "set-cookie",
                "api_key",
                "access_token",
                "refresh_token",
                "password",
            ]
            .contains(&key.to_ascii_lowercase().as_str())
            {
                *value = json!("[redacted]");
            } else {
                *value = sanitize(std::mem::take(value));
            }
        }
    } else if let Some(array) = value.as_array_mut() {
        for item in array {
            *item = sanitize(std::mem::take(item));
        }
    } else if let Some(text) = value.as_str()
        && text.len() > 32768
    {
        value = json!(format!(
            "{}… [truncated]",
            text.chars().take(8192).collect::<String>()
        ));
    }
    value
}

pub async fn page(State(app): State<Arc<App>>) -> Result<Html<String>, AppError> {
    let store = app.store.clone();
    let metas = tokio::task::spawn_blocking(move || store.scan()).await?;
    Ok(Html(app.render.render("capture.html", minijinja::context! { total => metas.len(), projects => crate::group_sessions(metas), capture_enabled => app.capture.enabled })?))
}
#[derive(Deserialize)]
pub struct Cursor {
    after: Option<u64>,
}
pub async fn records(
    State(app): State<Arc<App>>,
    Query(cursor): Query<Cursor>,
) -> Result<Json<Value>, StatusCode> {
    if !app.capture.enabled {
        return Err(StatusCode::FORBIDDEN);
    }
    let buffer = app.capture.buffer.lock().unwrap();
    let entries: Vec<_> = buffer
        .entries
        .iter()
        .filter(|e| e.seq > cursor.after.unwrap_or(0))
        .take(500)
        .collect();
    let next = entries
        .last()
        .map(|e| e.seq)
        .unwrap_or(cursor.after.unwrap_or(0));
    Ok(Json(
        json!({"running":app.capture.running.load(Ordering::SeqCst),"entries":entries,"next":next,"dropped":buffer.dropped,"capacity":CAPACITY}),
    ))
}
#[derive(Deserialize)]
pub struct Control {
    action: String,
}
pub async fn control(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(control): Json<Control>,
) -> Result<Json<Value>, StatusCode> {
    if !app.capture.enabled {
        return Err(StatusCode::FORBIDDEN);
    }
    let host = headers
        .get("host")
        .and_then(|s| s.to_str().ok())
        .ok_or(StatusCode::FORBIDDEN)?;
    if headers
        .get("origin")
        .and_then(|s| s.to_str().ok())
        .is_some_and(|s| s != format!("http://{host}"))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    match control.action.as_str() {
        "start" => app.capture.start(app.store.clone()),
        "stop" => {
            app.capture
                .record("internal", "", "capture.stopped", json!({}));
            app.capture.running.store(false, Ordering::SeqCst);
            app.capture.generation.fetch_add(1, Ordering::SeqCst);
        }
        "clear" => {
            let mut buffer = app.capture.buffer.lock().unwrap();
            buffer.entries.clear();
            buffer.dropped = 0;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    Ok(Json(
        json!({"running":app.capture.running.load(Ordering::SeqCst)}),
    ))
}

pub async fn http_trace(
    State(app): State<Arc<App>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path().to_owned();
    let method = request.method().to_string();
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    // Exclude inspector polling to prevent the observer flooding its own buffer.
    if !path.starts_with("/capture/") {
        app.capture.record("http", "", "http.response", json!({"method":method,"path":path,"status":response.status().as_u16(),"headers_ready_ms":started.elapsed().as_millis(),"scope":"AgentBanter HTTP only; request and response bodies not captured"}));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn socket_inventory_keeps_process_and_transport() {
        let sockets = parse_sockets(
            "p42\ncCodex\nf8\ntIPv4\nn127.0.0.1:7000->127.0.0.1:8000\nf9\ntunix\nn/tmp/example.sock\n",
        );
        assert_eq!(sockets.len(), 2);
        assert_eq!(sockets[0]["process"], "Codex");
        assert_eq!(sockets[1]["type"], "unix");
    }
    #[test]
    fn buffer_is_bounded_and_sanitized() {
        let capture = Capture::new(true);
        capture.running.store(true, Ordering::SeqCst);
        for _ in 0..4001 {
            capture.record(
                "test",
                "",
                "record",
                json!({"password":"secret","nested":{"type":"reasoning","content":"private"}}),
            );
        }
        let buffer = capture.buffer.lock().unwrap();
        assert_eq!(buffer.entries.len(), 4000);
        assert_eq!(buffer.dropped, 1);
        let text = buffer.entries.back().unwrap().detail.to_string();
        assert!(!text.contains("secret"));
        assert!(!text.contains("private"));
    }
}
