//! Finds transcripts under ~/.claude/projects, parses the ones being viewed, and tails them for changes.

use crate::model::{Session, user_text};
use crate::text::{clean_prompt, first_line, fmt_age, truncate};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};
use tokio::sync::broadcast;

#[derive(Clone, Serialize, Debug)]
pub struct SessionMeta {
    pub id: String,
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
    tail: Mutex<TailState>,
}

#[derive(Default)]
struct TailState {
    offset: u64,
    partial: Vec<u8>,
}

pub struct Store {
    projects_dir: PathBuf,
    index: Mutex<HashMap<PathBuf, (SystemTime, u64, SessionMeta)>>,
    sessions: Mutex<HashMap<String, Arc<Loaded>>>,
    pub tx: broadcast::Sender<Update>,
}

impl Store {
    pub fn new(projects_dir: PathBuf) -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        Arc::new(Self {
            projects_dir,
            index: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            tx,
        })
    }

    /// All sessions, newest first. Peeks only at file heads/tails, cached by (mtime, size).
    pub fn scan(&self) -> Vec<SessionMeta> {
        let mut out = Vec::new();
        let Ok(projects) = fs::read_dir(&self.projects_dir) else {
            return out;
        };
        let now = SystemTime::now();
        let mut index = self.index.lock().unwrap();
        for p in projects.flatten() {
            let pdir = p.path();
            if !pdir.is_dir() {
                continue;
            }
            let slug = p.file_name().to_string_lossy().to_string();
            let Ok(files) = fs::read_dir(&pdir) else {
                continue;
            };
            for f in files.flatten() {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let Ok(md) = f.metadata() else { continue };
                let (mtime, size) = (md.modified().unwrap_or(now), md.len());
                if size == 0 {
                    continue;
                }
                let mut meta = match index.get(&path) {
                    Some((m, s, meta)) if *m == mtime && *s == size => meta.clone(),
                    _ => {
                        let meta = peek(&path, &slug, mtime, size);
                        index.insert(path.clone(), (mtime, size, meta.clone()));
                        meta
                    }
                };
                let age = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
                meta.live = age < 180;
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
            ingest_bytes(&mut session, &mut tail.partial, &bytes);
            tail.offset = bytes.len() as u64;
            Ok(Arc::new(Loaded {
                id: id_s,
                path: meta.path,
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
        if len == tail.offset {
            return false;
        }
        if len < tail.offset {
            // Truncated or rewritten: rebuild from scratch.
            let bytes = fs::read(&self.path).unwrap_or_default();
            let mut s = Session::new(&self.id);
            *tail = TailState::default();
            ingest_bytes(&mut s, &mut tail.partial, &bytes);
            tail.offset = bytes.len() as u64;
            *self.session.write().unwrap() = s;
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
        ingest_bytes(&mut s, &mut tail.partial, &buf);
        tail.offset += buf.len() as u64;
        s.events.len() != before
    }
}

fn ingest_bytes(session: &mut Session, partial: &mut Vec<u8>, bytes: &[u8]) {
    partial.extend_from_slice(bytes);
    let mut start = 0;
    while let Some(pos) = partial[start..].iter().position(|&b| b == b'\n') {
        if let Ok(line) = std::str::from_utf8(&partial[start..start + pos]) {
            session.ingest_line(line);
        }
        start += pos + 1;
    }
    partial.drain(..start);
}

/// Cheap metadata: cwd + first prompt from the head, latest AI title from the tail.
fn peek(path: &Path, slug: &str, mtime: SystemTime, size: u64) -> SessionMeta {
    const CHUNK: u64 = 96 * 1024;
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (mut cwd, mut first_prompt, mut title) = (String::new(), String::new(), String::new());

    if let Ok(mut f) = fs::File::open(path) {
        let mut head = vec![0u8; CHUNK.min(size) as usize];
        if f.read_exact(&mut head).is_ok() {
            for line in String::from_utf8_lossy(&head).lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
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
