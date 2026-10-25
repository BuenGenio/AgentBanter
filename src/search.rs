//! Bounded, cached search over the same normalized blocks used by the timeline.
use crate::{
    App,
    model::{Event, Session},
    source::{self, Source},
    store::SessionMeta,
};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};

const FILE_LIMIT: u64 = 64 * 1024 * 1024;
const TEXT_LIMIT: usize = 300_000;
const CACHE_LIMIT: usize = 96 * 1024 * 1024;
#[derive(Default)]
pub struct Search {
    cache: Mutex<HashMap<PathBuf, Cached>>,
}
struct Cached {
    stamp: SystemTime,
    size: u64,
    blocks: Vec<Block>,
    bytes: usize,
    limited: bool,
}
struct Block {
    anchor: String,
    kind: &'static str,
    text: String,
}
#[derive(Deserialize, Default)]
pub struct Parameters {
    q: Option<String>,
    scope: Option<String>,
    agent: Option<String>,
    kind: Option<String>,
    regex: Option<bool>,
    case: Option<bool>,
}

fn plain(html: &str) -> String {
    static TAGS: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new("<[^>]*>").unwrap());
    TAGS.replace_all(html, " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}
fn blocks(session: &Session) -> (Vec<Block>, bool) {
    let mut out = Vec::<Block>::new();
    let mut tools = HashMap::new();
    let mut limited = false;
    for event in &session.events {
        let entry = match event {
            Event::Prompt { id, html, .. } => Some((id.clone(), "prompt", plain(html))),
            Event::Narrative { id, html, .. } => Some((id.clone(), "reply", plain(html))),
            Event::Reasoning {
                id, label, html, ..
            } => Some((id.clone(), "reasoning", format!("{label}\n{}", plain(html)))),
            Event::Decision(d) => Some((
                d.id.clone(),
                if d.source == "inferred" {
                    "inferred"
                } else {
                    "decision"
                },
                format!("{}\n{}", d.question, d.answer),
            )),
            Event::Tool(t) => {
                tools.insert(t.id.clone(), out.len());
                Some((
                    format!("tool-{}", t.id),
                    if t.icon == "bash" || t.detail.contains("exec_command") {
                        "command"
                    } else {
                        "tool"
                    },
                    format!("{}\n{}\n{}", t.name, t.summary, t.detail),
                ))
            }
            Event::ToolResult(r) => {
                if let Some(&i) = tools.get(&r.tool_id) {
                    let text = &mut out[i].text;
                    let extra = format!("\n{}\n{}", r.detail, r.raw);
                    let remaining = TEXT_LIMIT.saturating_sub(text.len());
                    if extra.len() <= remaining {
                        text.push_str(&extra);
                    } else {
                        let mut end = remaining;
                        while end > 0 && !extra.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.push_str(&extra[..end]);
                        limited = true;
                    }
                }
                None
            }
            _ => None,
        };
        if let Some((anchor, kind, text)) = entry {
            let text = if text.len() > TEXT_LIMIT {
                limited = true;
                {
                    let mut end = TEXT_LIMIT;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text[..end].to_owned()
                }
            } else {
                text
            };
            out.push(Block { anchor, kind, text });
        }
    }
    (out, limited)
}
fn read(meta: &SessionMeta) -> anyhow::Result<(Vec<Block>, bool)> {
    let file = fs::File::open(&meta.path)?;
    let mut reader = BufReader::new(file.take(FILE_LIMIT));
    let mut bytes = Vec::new();
    let mut session = Session::new(&meta.id);
    let mut number = 0;
    while reader.read_until(b'\n', &mut bytes)? > 0 {
        number += 1;
        if bytes.last() != Some(&b'\n') {
            break;
        }
        if let Ok(line) = std::str::from_utf8(&bytes) {
            if meta.source == Source::Codex {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    for item in source::normalize(meta.source, &value, &format!("line-{number}")) {
                        session.ingest(&item);
                    }
                }
            } else {
                session.ingest_line(line);
            }
        }
        bytes.clear();
    }
    Ok(blocks(&session))
}
fn matcher(q: &str, regex: bool, case: bool) -> Result<Regex, String> {
    if q.chars().count() > 512 {
        return Err("Search is limited to 512 characters.".into());
    }
    RegexBuilder::new(&if regex { q.into() } else { regex::escape(q) })
        .case_insensitive(!case)
        .size_limit(1024 * 1024)
        .build()
        .map_err(|e| format!("Invalid search pattern: {e}"))
}
fn excerpt(text: &str, found: usize) -> String {
    let start = text[..found]
        .char_indices()
        .rev()
        .nth(65)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut out = if start > 0 {
        "…".to_owned()
    } else {
        String::new()
    };
    out.extend(text[start..].chars().take(230));
    if text[start..].chars().count() > 230 {
        out.push('…');
    }
    out
}
impl Search {
    fn run(&self, metas: Vec<SessionMeta>, p: Parameters) -> Result<Value, String> {
        let q = p.q.unwrap_or_default();
        let kind = p.kind.unwrap_or_else(|| "all".into());
        let matcher = matcher(&q, p.regex.unwrap_or(false), p.case.unwrap_or(false))?;
        let allowed = if let Some(scope) = &p.scope {
            if !metas.iter().any(|m| &m.id == scope) {
                return Err("Conversation not found.".into());
            }
            crate::team::members(&metas, scope)
                .into_iter()
                .map(|m| m.id.clone())
                .collect::<HashSet<_>>()
        } else {
            metas.iter().map(|m| m.id.clone()).collect()
        };
        if q.trim().is_empty() && p.scope.is_none() {
            return Ok(json!({"hits":[],"sessions":[],"count":0,"anchors":[],"warnings":[]}));
        }
        let mut cache = self.cache.lock().unwrap();
        let mut hits = Vec::new();
        let mut matched_sessions = Vec::new();
        let mut anchors = Vec::new();
        let mut count = 0;
        let mut warnings = Vec::new();
        for meta in metas.iter().filter(|m| {
            allowed.contains(&m.id) && p.agent.as_ref().is_none_or(|a| a == "all" || a == &m.id)
        }) {
            let Ok(info) = fs::metadata(&meta.path) else {
                warnings.push(format!("Could not read {}", meta.title));
                continue;
            };
            if info.len() > FILE_LIMIT {
                warnings.push(format!("Skipped {}: log exceeds 64 MiB.", meta.title));
                continue;
            }
            let stamp = info.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let rebuild = cache
                .get(&meta.path)
                .is_none_or(|c| c.stamp != stamp || c.size != info.len());
            if rebuild {
                let (mut blocks, mut limited) = match read(meta) {
                    Ok(value) => value,
                    Err(_) => {
                        warnings.push(format!("Could not index {}", meta.title));
                        continue;
                    }
                };
                let mut bytes = 0;
                let keep = blocks
                    .iter()
                    .take_while(|b| {
                        bytes += b.text.len();
                        bytes <= 16 * 1024 * 1024
                    })
                    .count();
                if keep < blocks.len() {
                    limited = true;
                    blocks.truncate(keep);
                }
                let bytes = blocks.iter().map(|b| b.text.len()).sum();
                if cache.values().map(|c| c.bytes).sum::<usize>() + bytes > CACHE_LIMIT {
                    cache.clear();
                }
                cache.insert(
                    meta.path.clone(),
                    Cached {
                        stamp,
                        size: info.len(),
                        blocks,
                        bytes,
                        limited,
                    },
                );
            }
            let entry = &cache[&meta.path];
            if entry.limited {
                warnings.push(format!(
                    "Very long records in {} were shortened for indexing.",
                    meta.title
                ));
            }
            let mut session_match = false;
            for block in entry.blocks.iter().rev() {
                if kind != "all"
                    && kind != block.kind
                    && !(kind == "tool" && block.kind == "command")
                {
                    continue;
                }
                if let Some(found) = matcher.find(&block.text) {
                    count += 1;
                    session_match = true;
                    if p.scope.as_ref() == Some(&meta.id) {
                        anchors.push(block.anchor.clone());
                    }
                    if hits.len() < 100 {
                        hits.push(json!({"session":meta.id,"title":meta.title,"agent":meta.agent_name.as_ref().unwrap_or(&meta.agent),"provider":meta.agent,"kind":block.kind,"anchor":block.anchor,"excerpt":excerpt(&block.text,found.start())}));
                    }
                }
            }
            if session_match {
                matched_sessions.push(meta.id.clone());
            }
        }
        Ok(
            json!({"hits":hits,"sessions":matched_sessions,"count":count,"anchors":anchors,"warnings":warnings,"limited":count>100}),
        )
    }
}
pub async fn query(State(app): State<Arc<App>>, Query(p): Query<Parameters>) -> Response {
    let store = app.store.clone();
    let search = app.search.clone();
    match tokio::task::spawn_blocking(move || search.run(store.scan(), p)).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (StatusCode::BAD_REQUEST, Json(json!({"error":error}))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Search could not complete."})),
        )
            .into_response(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(id: &str, parent: Option<&str>, text: &str) -> SessionMeta {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agentbanter-search-{}-{n}.jsonl",
            std::process::id()
        ));
        let line = json!({"type":"user","uuid":format!("{id}-prompt"),"message":{"content":text}});
        fs::write(&path, format!("{line}\n")).unwrap();
        SessionMeta {
            id: id.into(),
            source: Source::Claude,
            agent: "Claude".into(),
            parent_id: parent.map(str::to_owned),
            agent_name: None,
            agent_task: None,
            path,
            slug: String::new(),
            cwd: "/same-project".into(),
            project: "test".into(),
            title: id.into(),
            mtime: chrono::Utc::now(),
            age: String::new(),
            size: String::new(),
            live: false,
        }
    }
    #[test]
    fn scope_follows_recorded_agents_and_excludes_project_neighbors() {
        let metas = vec![
            fixture("root", None, "needle"),
            fixture("child", Some("root"), "needle"),
            fixture("other", None, "needle"),
        ];
        let result = Search::default()
            .run(
                metas.clone(),
                Parameters {
                    q: Some("needle".into()),
                    scope: Some("root".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(result["count"], 2);
        assert_eq!(result["anchors"].as_array().unwrap().len(), 1);
        let child = Search::default()
            .run(
                metas.clone(),
                Parameters {
                    q: Some("needle".into()),
                    scope: Some("root".into()),
                    agent: Some("child".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(child["count"], 1);
        assert_eq!(child["hits"][0]["session"], "child");
        assert_eq!(child["anchors"], json!([]));
        for m in metas {
            fs::remove_file(m.path).unwrap();
        }
    }
    #[test]
    fn appended_records_invalidate_cache_and_preview_limit_keeps_all_anchors() {
        let meta = fixture("root", None, "first");
        let search = Search::default();
        let params = || Parameters {
            q: Some("needle".into()),
            scope: Some("root".into()),
            ..Default::default()
        };
        assert_eq!(
            search.run(vec![meta.clone()], params()).unwrap()["count"],
            0
        );
        let lines=(0..120).map(|i|format!("{}\n",json!({"type":"user","uuid":format!("p{i}"),"message":{"content":format!("needle {i}")}}))).collect::<String>();
        fs::write(&meta.path, lines).unwrap();
        let result = search.run(vec![meta.clone()], params()).unwrap();
        assert_eq!(result["count"], 120);
        assert_eq!(result["hits"].as_array().unwrap().len(), 100);
        assert_eq!(result["anchors"].as_array().unwrap().len(), 120);
        assert_eq!(result["limited"], true);
        fs::remove_file(meta.path).unwrap();
    }
    #[test]
    fn literal_regex_and_case_are_explicit() {
        assert!(matcher("a.b", false, false).unwrap().is_match("A.B"));
        assert!(!matcher("a.b", false, false).unwrap().is_match("axb"));
        assert!(matcher("a.b", true, true).unwrap().is_match("axb"));
        assert!(matcher("[", true, false).is_err());
    }
    #[test]
    fn unicode_excerpt_has_valid_boundaries() {
        let text = "🌈".repeat(100) + "needle";
        assert!(excerpt(&text, 400).contains("needle"));
    }
    #[test]
    fn tools_and_results_share_the_real_timeline_anchor() {
        let mut s = Session::new("test");
        for value in [
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Bash","input":{"command":"echo hello"}}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":"needle in output"}]}}),
        ] {
            s.ingest(&value);
        }
        let (b, _) = blocks(&s);
        assert!(
            b.iter()
                .any(|b| b.anchor == "tool-a" && b.text.contains("needle in output"))
        );
    }
}
