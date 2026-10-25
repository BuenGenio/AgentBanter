//! Finds transcripts under ~/.claude/projects, parses the ones being viewed, and tails them for changes.

use crate::model::{Session, user_text};
use crate::source::{self, Source};
use crate::text::{clean_prompt, first_line, fmt_age, truncate};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};
use tokio::sync::broadcast;

#[derive(Clone, Serialize, Debug)]
pub struct SessionMeta {
    pub id: String,
    pub source: Source,
    pub agent: String,
    pub parent_id: Option<String>,
    pub agent_name: Option<String>,
    pub agent_task: Option<String>,
    #[serde(skip)]
    pub path: PathBuf,
    pub slug: String,
    pub cwd: String,
    pub project: String,
    pub title: String,
    pub mtime: DateTime<Utc>,
    pub age: String,
    pub size: String,
    pub live: bool,
}

#[derive(Clone, Debug)]
pub struct Update {
    pub session_id: String,
}

pub struct Loaded {
    pub id: String,
    pub path: PathBuf,
    pub session: RwLock<Session>,
    pub source: Source,
    pub generation: AtomicU64,
    tail: Mutex<TailState>,
}

#[derive(Default)]
struct TailState {
    offset: u64,
    partial: Vec<u8>,
    modified: Option<SystemTime>,
    line: u64,
}

pub struct Store {
    roots: Vec<(Source, PathBuf)>,
    index: Mutex<HashMap<PathBuf, (SystemTime, u64, SessionMeta)>>,
    sessions: Mutex<HashMap<String, Arc<Loaded>>>,
    pub tx: broadcast::Sender<Update>,
}

impl Store {
    pub fn new(roots: Vec<(Source, PathBuf)>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        Arc::new(Self {
            roots,
            index: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            tx,
        })
    }

    /// All sessions, newest first. Peeks only at file heads/tails, cached by (mtime, size).
    pub fn scan(&self) -> Vec<SessionMeta> {
        let mut out = Vec::new();
        let now = SystemTime::now();
        let mut index = self.index.lock().unwrap();
        for (source, root) in &self.roots {
            let mut paths = Vec::new();
            collect_transcripts(root, *source, 0, &mut paths);
            for path in paths {
                let Ok(md) = fs::metadata(&path) else {
                    continue;
                };
                let (mtime, size) = (md.modified().unwrap_or(now), md.len());
                if size == 0 {
                    continue;
                }
                let slug = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .unwrap_or_default()
                    .to_string_lossy();
                let mut meta = match index.get(&path) {
                    Some((m, s, meta)) if *m == mtime && *s == size => meta.clone(),
                    _ => {
                        let meta = peek(&path, &slug, *source, mtime, size);
                        index.insert(path.clone(), (mtime, size, meta.clone()));
                        meta
                    }
                };
                let age = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
                meta.live = *source != Source::Chatgpt && age < 180;
                meta.age = fmt_age(age);
                out.push(meta);
            }
        }
        out.sort_by_key(|m| std::cmp::Reverse(m.mtime));
        out
    }

    /// Fully parse a session (once) and keep it tailed.
    pub async fn load(self: &Arc<Self>, id: &str) -> Result<Arc<Loaded>> {
        if let Some(l) = self.sessions.lock().unwrap().get(id) {
            return Ok(l.clone());
        }
        let store = self.clone();
        let id_s = id.to_string();
        let loaded = tokio::task::spawn_blocking(move || -> Result<Arc<Loaded>> {
            let meta = store
                .scan()
                .into_iter()
                .find(|m| m.id == id_s)
                .context("session not found")?;
            let mut session = Session::new(&id_s);
            let mut tail = TailState::default();
            let bytes = fs::read(&meta.path)?;
            ingest_bytes(&mut session, &mut tail, &bytes, meta.source);
            tail.modified = fs::metadata(&meta.path)
                .ok()
                .and_then(|m| m.modified().ok());
            tail.offset = bytes.len() as u64;
            Ok(Arc::new(Loaded {
                id: id_s,
                path: meta.path,
                source: meta.source,
                generation: AtomicU64::new(0),
                session: RwLock::new(session),
                tail: Mutex::new(tail),
            }))
        })
        .await??;
        let mut map = self.sessions.lock().unwrap();
        Ok(map.entry(id.to_string()).or_insert(loaded).clone())
    }

    /// Poll loaded sessions for appended bytes and broadcast when new events appear.
    pub fn spawn_tailer(self: &Arc<Self>, poll: Duration) {
        let store = self.clone();
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(poll);
            loop {
                iv.tick().await;
                let loaded: Vec<Arc<Loaded>> =
                    store.sessions.lock().unwrap().values().cloned().collect();
                for l in loaded {
                    let l2 = l.clone();
                    let changed = tokio::task::spawn_blocking(move || l2.poll())
                        .await
                        .unwrap_or(false);
                    if changed {
                        let _ = store.tx.send(Update {
                            session_id: l.id.clone(),
                        });
                    }
                }
            }
        });
    }
}

impl Loaded {
    fn poll(&self) -> bool {
        let Ok(md) = fs::metadata(&self.path) else {
            return false;
        };
        let len = md.len();
        let mut tail = self.tail.lock().unwrap();
        let modified = md.modified().ok();
        if len == tail.offset && modified == tail.modified {
            return false;
        }
        if len < tail.offset || self.source == Source::Chatgpt || len == tail.offset {
            // Truncated or rewritten: rebuild from scratch.
            let Ok(bytes) = fs::read(&self.path) else {
                return false;
            };
            let mut s = Session::new(&self.id);
            *tail = TailState::default();
            ingest_bytes(&mut s, &mut tail, &bytes, self.source);
            tail.modified = modified;
            tail.offset = bytes.len() as u64;
            *self.session.write().unwrap() = s;
            self.generation.fetch_add(1, Ordering::SeqCst);
            return true;
        }
        let Ok(mut f) = fs::File::open(&self.path) else {
            return false;
        };
        if f.seek(SeekFrom::Start(tail.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::with_capacity((len - tail.offset) as usize);
        if f.read_to_end(&mut buf).is_err() {
            return false;
        }
        let mut s = self.session.write().unwrap();
        let before = s.events.len();
        ingest_bytes(&mut s, &mut tail, &buf, self.source);
        tail.modified = modified;
        tail.offset += buf.len() as u64;
        s.events.len() != before
    }
}

fn ingest_bytes(session: &mut Session, tail: &mut TailState, bytes: &[u8], source: Source) {
    let partial = &mut tail.partial;
    partial.extend_from_slice(bytes);
    let mut start = 0;
    while let Some(pos) = partial[start..].iter().position(|&b| b == b'\n') {
        if let Ok(line) = std::str::from_utf8(&partial[start..start + pos]) {
            if source != Source::Codex {
                session.ingest_line(line);
                start += pos + 1;
                continue;
            }
            tail.line += 1;
            match serde_json::from_str::<Value>(line) {
                Ok(v) => {
                    for record in source::normalize(source, &v, &format!("line-{}", tail.line)) {
                        session.ingest(&record);
                    }
                }
                Err(_) if !line.trim().is_empty() => session.bad_lines += 1,
                _ => {}
            }
        }
        start += pos + 1;
    }
    partial.drain(..start);
}

fn collect_transcripts(dir: &Path, source: Source, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        } else if kind.is_dir() && depth < if source == Source::Claude { 3 } else { 4 } {
            collect_transcripts(&path, source, depth + 1, out);
        }
    }
}

/// Cheap metadata: cwd + first prompt from the head, latest AI title from the tail.
fn peek(path: &Path, slug: &str, source: Source, mtime: SystemTime, size: u64) -> SessionMeta {
    const CHUNK: u64 = 96 * 1024;
    let native_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let codex_native =
        if source == Source::Codex && native_id.starts_with("rollout-") && native_id.len() >= 36 {
            let suffix = native_id.get(native_id.len() - 36..).unwrap_or(&native_id);
            if suffix.bytes().filter(|&b| b == b'-').count() == 4 {
                suffix
            } else {
                &native_id
            }
        } else {
            &native_id
        };
    let mut id = source.session_id(codex_native);
    let mut parent_id = None;
    let mut agent_name = None;
    let mut agent_task = None;
    if source == Source::Claude
        && path
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == "subagents")
    {
        parent_id = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned());
        if let Some(parent) = &parent_id {
            id = format!("claude-agent--{parent}--{native_id}");
            agent_name = Some(native_id.clone());
        }
    }
    let (mut cwd, mut first_prompt, mut title) = (String::new(), String::new(), String::new());

    if let Ok(mut f) = fs::File::open(path) {
        let head_limit = if source == Source::Codex {
            2 * 1024 * 1024
        } else {
            CHUNK
        };
        let mut head = vec![0u8; head_limit.min(size) as usize];
        if f.read_exact(&mut head).is_ok() {
            for line in String::from_utf8_lossy(&head).lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if source == Source::Ollama {
                    if let Some(model) = v["local_model"].as_str() {
                        agent_name = Some(if v["local_role"] == "trace analyst" {
                            format!("{model} · analyst")
                        } else {
                            model.to_owned()
                        });
                    }
                    if let Some(parent) = v["local_parent"].as_str() {
                        parent_id = Some(parent.into());
                        agent_task =
                            Some("Analysis of recorded thinking, generated afterward".into());
                    }
                }
                if source == Source::Codex && v["type"] == "session_meta" {
                    let spawn = &v["payload"]["source"]["subagent"]["thread_spawn"];
                    agent_task = spawn["agent_role"]
                        .as_str()
                        .or_else(|| spawn["agent_path"].as_str())
                        .map(str::to_owned);
                    parent_id = spawn["parent_thread_id"]
                        .as_str()
                        .map(|p| source.session_id(p));
                    agent_name = spawn["agent_nickname"]
                        .as_str()
                        .or_else(|| spawn["agent_path"].as_str())
                        .map(str::to_owned);

                    if let Some(native) = v["payload"]["id"]
                        .as_str()
                        .or_else(|| v["payload"]["session_id"].as_str())
                    {
                        id = source.session_id(native);
                    }
                }
                for v in source::normalize(source, &v, "peek") {
                    if cwd.is_empty()
                        && let Some(c) = v["cwd"].as_str()
                    {
                        cwd = c.to_string();
                    }
                    if first_prompt.is_empty()
                        && v["type"] == "user"
                        && v["isMeta"] != true
                        && let Some(p) = user_text(&v).and_then(|t| clean_prompt(&t))
                    {
                        first_prompt = truncate(first_line(&p), 90);
                    }
                    if v["type"] == "ai-title"
                        && let Some(t) = v["aiTitle"].as_str()
                    {
                        title = t.to_string();
                    }
                }
            }
        }
        if size > CHUNK {
            let mut tail = vec![0u8; CHUNK as usize];
            if f.seek(SeekFrom::Start(size - CHUNK)).is_ok() && f.read_exact(&mut tail).is_ok() {
                for line in String::from_utf8_lossy(&tail).lines().skip(1) {
                    if !line.contains("\"ai-title\"") {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(line)
                        && let Some(t) = v["aiTitle"].as_str()
                    {
                        title = t.to_string();
                    }
                }
            }
        }
    }
    if cwd.is_empty() {
        cwd = slug.replacen('-', "/", 1).replace('-', "/");
    }
    let project = cwd.rsplit('/').next().unwrap_or(&cwd).to_string();
    let title = if !title.is_empty() {
        title
    } else if !first_prompt.is_empty() {
        first_prompt
    } else {
        id.clone()
    };
    SessionMeta {
        id,
        source,
        agent: source.label().to_string(),
        parent_id,
        agent_name,
        agent_task,
        path: path.to_path_buf(),
        slug: slug.to_string(),
        cwd,
        project,
        title,
        mtime: DateTime::<Utc>::from(mtime),
        age: String::new(),
        size: if size >= 1 << 20 {
            format!("{:.1} MB", size as f64 / 1048576.0)
        } else {
            format!("{} KB", size.div_ceil(1024))
        },
        live: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Event;
    use std::io::Write;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "agentbanter-test-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_explicit_team_metadata_and_namespaces_claude_children() {
        let temp = Temp::new();
        let claude = temp.0.join("claude/project/parent/subagents");
        let codex = temp.0.join("codex");
        fs::create_dir_all(&claude).unwrap();
        fs::create_dir_all(&codex).unwrap();
        fs::write(claude.join("agent-child.jsonl"), "{\"type\":\"user\",\"sessionId\":\"parent\",\"agentId\":\"child\",\"cwd\":\"/project\",\"message\":{\"content\":\"Review code\"}}\n").unwrap();
        fs::write(codex.join("rollout.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"source\":{\"subagent\":{\"thread_spawn\":{\"parent_thread_id\":\"parent\",\"agent_nickname\":\"Reviewer\"}}}}}\n").unwrap();
        let store = Store::new(vec![
            (Source::Claude, temp.0.join("claude")),
            (Source::Codex, codex),
        ]);
        let all = store.scan();
        let c = all.iter().find(|m| m.source == Source::Claude).unwrap();
        assert_eq!(c.id, "claude-agent--parent--agent-child");
        assert_eq!(c.parent_id.as_deref(), Some("parent"));
        let c = all.iter().find(|m| m.source == Source::Codex).unwrap();
        assert_eq!(c.parent_id.as_deref(), Some("codex--parent"));
        assert_eq!(c.agent_name.as_deref(), Some("Reviewer"));
    }

    #[tokio::test]
    async fn discovers_sources_and_tails_partial_codex_records() {
        let temp = Temp::new();
        let codex = temp.0.join("codex/2026/10/21");
        let claude = temp.0.join("claude/project");
        fs::create_dir_all(&codex).unwrap();
        fs::create_dir_all(&claude).unwrap();
        let path = codex.join("rollout-example.jsonl");
        fs::write(
            &path,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"same\",\"cwd\":\"/project\"}}\n",
        )
        .unwrap();
        fs::write(
            claude.join("same.jsonl"),
            "{\"type\":\"user\",\"uuid\":\"u\",\"message\":{\"content\":\"Claude prompt\"}}\n",
        )
        .unwrap();
        let store = Store::new(vec![
            (Source::Codex, temp.0.join("codex")),
            (Source::Claude, temp.0.join("claude")),
        ]);
        let ids = store.scan().into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert!(ids.contains(&"same".into()) && ids.contains(&"codex--same".into()));
        let loaded = store.load("codex--same").await.unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        write!(file,"{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"New prompt\"}}]}}}}").unwrap();
        assert!(!loaded.poll());
        writeln!(file).unwrap();
        assert!(loaded.poll());
        assert_eq!(
            loaded
                .session
                .read()
                .unwrap()
                .events
                .iter()
                .filter(|e| matches!(e, Event::Prompt { .. }))
                .count(),
            1
        );
        assert!(!loaded.poll());
    }

    #[tokio::test]
    async fn reimport_replaces_snapshot_and_advances_generation() {
        let temp = Temp::new();
        let source = temp.0.join("export.json");
        let destination = temp.0.join("chatgpt");
        let fixture = include_str!("../fixtures/chatgpt/conversations.json");
        fs::write(&source, fixture).unwrap();
        crate::chatgpt::import(&source, &destination).unwrap();
        let store = Store::new(vec![(Source::Chatgpt, destination.clone())]);
        assert!(!store.scan()[0].live);
        let loaded = store.load("chatgpt--chatgpt-demo").await.unwrap();
        let path = destination.join("chatgpt-demo.jsonl");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        crate::chatgpt::import(&source, &destination).unwrap();
        assert_eq!(before, fs::metadata(&path).unwrap().modified().unwrap());
        fs::write(
            &source,
            fixture.replace("I'd recommend", "An updated answer: I'd recommend"),
        )
        .unwrap();
        crate::chatgpt::import(&source, &destination).unwrap();
        assert!(loaded.poll());
        assert_eq!(loaded.generation.load(Ordering::SeqCst), 1);
        let session = loaded.session.read().unwrap();
        assert_eq!(
            session
                .events
                .iter()
                .filter(|e| matches!(e, Event::Narrative { .. }))
                .count(),
            1
        );
        assert!(
            session
                .events
                .iter()
                .any(|e| matches!(e,Event::Narrative{html,..} if html.contains("updated answer")))
        );
    }
}
