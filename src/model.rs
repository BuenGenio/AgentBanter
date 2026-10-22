//! Reduces raw transcript records into a timeline of prompts, reasoning, tool steps and decisions.

use crate::text::{
    clean_prompt, first_line, first_sentence, fmt_duration, infer_decisions, markdown, truncate,
};
use crate::tools;
use chrono::{DateTime, Local, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

static DECISION_DOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(adr|decisions?)\b").unwrap());

#[derive(Serialize, Clone, Debug)]
pub struct ToolView {
    pub id: String,
    pub step_id: String,
    pub ts: String,
    pub t: String,
    pub name: String,
    pub icon: String,
    pub summary: String,
    pub detail: String,
    pub files: Vec<String>,
    pub result: Option<ResultView>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ResultView {
    pub tool_id: String,
    pub t: String,
    pub ok: bool,
    pub summary: String,
    pub detail: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct DecisionView {
    pub id: String,
    pub ts: String,
    pub t: String,
    /// explicit (answered question), inferred (heuristic from prose), document (ADR-like file written)
    pub source: String,
    pub question: String,
    pub answer: String,
    pub prompt_n: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Prompt {
        id: String,
        ts: String,
        t: String,
        n: usize,
        html: String,
    },
    Narrative {
        id: String,
        ts: String,
        t: String,
        html: String,
    },
    Reasoning {
        id: String,
        ts: String,
        t: String,
        label: String,
        html: String,
    },
    StepOpen {
        id: String,
        ts: String,
        t: String,
    },
    Tool(ToolView),
    ToolResult(ResultView),
    Decision(DecisionView),
}

/// What the templates iterate over: events with tools folded into their step and stats attached to prompts.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Prompt {
        id: String,
        ts: String,
        t: String,
        n: usize,
        html: String,
        stats: PromptStats,
    },
    Narrative {
        id: String,
        ts: String,
        t: String,
        html: String,
    },
    Reasoning {
        id: String,
        ts: String,
        t: String,
        label: String,
        html: String,
    },
    Step {
        id: String,
        ts: String,
        t: String,
        tools: Vec<ToolView>,
    },
    Decision(DecisionView),
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct PromptStats {
    pub tools: usize,
    pub commands: usize,
    pub files: Vec<String>,
    pub reasoning: usize,
    pub decisions: usize,
    pub errors: usize,
    pub duration: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct SessionStats {
    pub prompts: usize,
    pub tools: usize,
    pub files: usize,
    pub reasoning: usize,
    pub decisions: usize,
    pub errors: usize,
    pub duration: String,
    pub started: String,
    pub last: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct SessionView {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub project: String,
    pub branch: String,
}

#[derive(Default)]
pub struct Session {
    pub id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub events: Vec<Event>,
    pub sidechain_records: usize,
    pub bad_lines: usize,
    ts: Vec<Option<DateTime<Utc>>>,
    cur_step: Option<String>,
    prompt_count: usize,
    seen_decisions: HashSet<String>,
}

fn parse_ts(v: &Value) -> Option<DateTime<Utc>> {
    v["timestamp"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

fn fmt_t(ts: Option<DateTime<Utc>>) -> String {
    ts.map(|d| d.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

fn fmt_ts(ts: Option<DateTime<Utc>>) -> String {
    ts.map(|d| d.to_rfc3339()).unwrap_or_default()
}

/// Human text of a user record (string content or text blocks), before cleanup.
pub fn user_text(v: &Value) -> Option<String> {
    match &v["message"]["content"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let t: Vec<&str> = items
                .iter()
                .filter(|i| i["type"] == "text")
                .filter_map(|i| i["text"].as_str())
                .collect();
            if t.is_empty() {
                None
            } else {
                Some(t.join("\n"))
            }
        }
        _ => None,
    }
}

fn content_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl Session {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            ..Default::default()
        }
    }

    fn push(&mut self, ev: Event, ts: Option<DateTime<Utc>>) -> usize {
        self.events.push(ev);
        self.ts.push(ts);
        self.events.len() - 1
    }

    pub fn ingest_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => self.ingest(&v),
            Err(_) => self.bad_lines += 1,
        }
    }

    pub fn ingest(&mut self, v: &Value) {
        if v["isSidechain"].as_bool() == Some(true) {
            self.sidechain_records += 1;
            return;
        }
        if let Some(c) = v["cwd"].as_str() {
            self.cwd.get_or_insert_with(|| c.to_string());
        }
        if let Some(b) = v["gitBranch"].as_str()
            && !b.is_empty()
        {
            self.branch = Some(b.to_string());
        }
        let ts = parse_ts(v);
        match v["type"].as_str().unwrap_or("") {
            "ai-title" => {
                if let Some(t) = v["aiTitle"].as_str() {
                    self.title = Some(t.to_string());
                }
            }
            "user" => self.ingest_user(v, ts),
            "assistant" => self.ingest_assistant(v, ts),
            _ => {}
        }
    }

    fn ingest_user(&mut self, v: &Value, ts: Option<DateTime<Utc>>) {
        if v["isMeta"].as_bool() == Some(true) {
            return;
        }
        if let Some(items) = v["message"]["content"].as_array() {
            for it in items.iter().filter(|i| i["type"] == "tool_result") {
                self.ingest_tool_result(it, v, ts);
            }
        }
        let Some(clean) = user_text(v).and_then(|t| clean_prompt(&t)) else {
            return;
        };
        self.cur_step = None;
        self.prompt_count += 1;
        let uuid = v["uuid"].as_str().unwrap_or("x");
        let ev = Event::Prompt {
            id: format!("p-{uuid}"),
            ts: fmt_ts(ts),
            t: fmt_t(ts),
            n: self.prompt_count,
            html: markdown(&clean),
        };
        self.push(ev, ts);
    }

    fn ingest_tool_result(&mut self, it: &Value, rec: &Value, ts: Option<DateTime<Utc>>) {
        let tool_id = it["tool_use_id"].as_str().unwrap_or("").to_string();
        let is_error = it["is_error"].as_bool().unwrap_or(false);
        let tur = &rec["toolUseResult"];
        let mut ok = !is_error;
        let (summary, detail);

        if let Some(answers) = tur.get("answers").and_then(Value::as_object) {
            // AskUserQuestion: each answer is an explicit decision.
            let mut parts = Vec::new();
            for (q, a) in answers {
                let a = a.as_str().unwrap_or("").to_string();
                parts.push(a.clone());
                self.add_decision(
                    "explicit",
                    q.clone(),
                    a,
                    &format!("{tool_id}-{}", parts.len()),
                    ts,
                );
            }
            summary = truncate(&format!("Answered: {}", parts.join(" · ")), 200);
            detail = serde_json::to_string_pretty(answers).unwrap_or_default();
        } else if tur.get("stdout").is_some() || tur.get("stderr").is_some() {
            let stdout = tur["stdout"].as_str().unwrap_or("");
            let stderr = tur["stderr"].as_str().unwrap_or("");
            if tur["interrupted"].as_bool() == Some(true) {
                ok = false;
            }
            let primary = if is_error || stdout.trim().is_empty() {
                stderr
            } else {
                stdout
            };
            let fl = first_line(primary);
            summary = if fl.is_empty() {
                "(no output)".into()
            } else {
                truncate(fl, 160)
            };
            detail = truncate(format!("{stdout}\n{stderr}").trim(), 6000);
        } else {
            let text = content_text(&it["content"]);
            let fl = first_line(&text);
            summary = if fl.is_empty() {
                "done".into()
            } else {
                truncate(fl, 160)
            };
            detail = truncate(&text, 6000);
        }
        let ev = Event::ToolResult(ResultView {
            tool_id,
            t: fmt_t(ts),
            ok,
            summary,
            detail,
        });
        self.push(ev, ts);
    }

    fn ingest_assistant(&mut self, v: &Value, ts: Option<DateTime<Utc>>) {
        let uuid = v["uuid"].as_str().unwrap_or("x").to_string();
        let Some(blocks) = v["message"]["content"].as_array() else {
            return;
        };
        for (i, b) in blocks.iter().enumerate() {
            let bid = format!("{uuid}-{i}");
            match b["type"].as_str().unwrap_or("") {
                "thinking" => {
                    let text = b["thinking"].as_str().unwrap_or("").trim();
                    if text.is_empty() {
                        continue;
                    }
                    self.cur_step = None;
                    let ev = Event::Reasoning {
                        id: format!("r-{bid}"),
                        ts: fmt_ts(ts),
                        t: fmt_t(ts),
                        label: first_sentence(text, 140),
                        html: markdown(text),
                    };
                    self.push(ev, ts);
                    self.infer(text, &bid, ts);
                }
                "text" => {
                    let text = b["text"].as_str().unwrap_or("").trim();
                    if text.is_empty() {
                        continue;
                    }
                    self.cur_step = None;
                    let ev = Event::Narrative {
                        id: format!("n-{bid}"),
                        ts: fmt_ts(ts),
                        t: fmt_t(ts),
                        html: markdown(text),
                    };
                    self.push(ev, ts);
                    self.infer(text, &bid, ts);
                }
                "tool_use" => self.ingest_tool_use(b, &bid, ts),
                _ => {}
            }
        }
    }

    fn ingest_tool_use(&mut self, b: &Value, bid: &str, ts: Option<DateTime<Utc>>) {
        let id = b["id"].as_str().unwrap_or(bid).to_string();
        let name = b["name"].as_str().unwrap_or("?").to_string();
        let step_id = match &self.cur_step {
            Some(s) => s.clone(),
            None => {
                let s = format!("s-{bid}");
                self.push(
                    Event::StepOpen {
                        id: s.clone(),
                        ts: fmt_ts(ts),
                        t: fmt_t(ts),
                    },
                    ts,
                );
                self.cur_step = Some(s.clone());
                s
            }
        };
        let sum = tools::summarize(&name, &b["input"], self.cwd.as_deref());
        // A decision record (ADR, DECISIONS.md) being written is itself a decision event.
        let content = if name == "Bash" {
            b["input"]["command"].as_str()
        } else {
            b["input"]["content"].as_str()
        }
        .unwrap_or("");
        for (i, f) in sum
            .files
            .iter()
            .filter(|f| DECISION_DOC.is_match(f))
            .enumerate()
        {
            let base = f.rsplit('/').next().unwrap_or(f);
            let after = if name == "Bash" {
                content
                    .find(base)
                    .map(|p| &content[p + base.len()..])
                    .unwrap_or("")
            } else {
                content
            };
            let heading = after
                .lines()
                .find(|l| l.starts_with('#'))
                .map(|l| l.trim_start_matches('#').trim().to_string());
            self.add_decision(
                "document",
                format!("Recorded in {f}"),
                heading.unwrap_or_else(|| f.clone()),
                &format!("doc-{bid}-{i}"),
                ts,
            );
        }
        let ev = Event::Tool(ToolView {
            id,
            step_id,
            ts: fmt_ts(ts),
            t: fmt_t(ts),
            name,
            icon: sum.icon.to_string(),
            summary: sum.summary,
            detail: sum.detail,
            files: sum.files,
            result: None,
        });
        self.push(ev, ts);
    }

    fn infer(&mut self, text: &str, bid: &str, ts: Option<DateTime<Utc>>) {
        for (i, s) in infer_decisions(text, 2).into_iter().enumerate() {
            self.add_decision("inferred", String::new(), s, &format!("{bid}-{i}"), ts);
        }
    }

    fn add_decision(
        &mut self,
        source: &str,
        question: String,
        answer: String,
        key: &str,
        ts: Option<DateTime<Utc>>,
    ) {
        let dedupe = format!("{question}|{answer}");
        if !self.seen_decisions.insert(dedupe) {
            return;
        }
        let ev = Event::Decision(DecisionView {
            id: format!("d-{key}"),
            ts: fmt_ts(ts),
            t: fmt_t(ts),
            source: source.to_string(),
            question,
            answer,
            prompt_n: self.prompt_count,
        });
        self.push(ev, ts);
    }

    // ---- views -------------------------------------------------------------

    pub fn view(&self) -> SessionView {
        let cwd = self.cwd.clone().unwrap_or_default();
        let project = cwd.rsplit('/').next().unwrap_or(&cwd).to_string();
        let title = self
            .title
            .clone()
            .or_else(|| self.first_prompt_text())
            .unwrap_or_else(|| self.id.clone());
        SessionView {
            id: self.id.clone(),
            title,
            cwd,
            project,
            branch: self.branch.clone().unwrap_or_default(),
        }
    }

    fn first_prompt_text(&self) -> Option<String> {
        self.events.iter().find_map(|e| match e {
            Event::Prompt { html, .. } => Some(truncate(first_line(&strip_tags(html)), 90)),
            _ => None,
        })
    }

    /// Block for a non-tool event (tools are folded into their step by `blocks`).
    pub fn block_at(&self, i: usize) -> Option<Block> {
        Some(match &self.events[i] {
            Event::Prompt { id, ts, t, n, html } => Block::Prompt {
                id: id.clone(),
                ts: ts.clone(),
                t: t.clone(),
                n: *n,
                html: html.clone(),
                stats: self.prompt_stats(i),
            },
            Event::Narrative { id, ts, t, html } => Block::Narrative {
                id: id.clone(),
                ts: ts.clone(),
                t: t.clone(),
                html: html.clone(),
            },
            Event::Reasoning {
                id,
                ts,
                t,
                label,
                html,
            } => Block::Reasoning {
                id: id.clone(),
                ts: ts.clone(),
                t: t.clone(),
                label: label.clone(),
                html: html.clone(),
            },
            Event::StepOpen { id, ts, t } => Block::Step {
                id: id.clone(),
                ts: ts.clone(),
                t: t.clone(),
                tools: Vec::new(),
            },
            Event::Decision(d) => Block::Decision(d.clone()),
            Event::Tool(_) | Event::ToolResult(_) => return None,
        })
    }

    pub fn blocks(&self) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        for (i, ev) in self.events.iter().enumerate() {
            match ev {
                Event::Tool(tv) => match out.last_mut() {
                    Some(Block::Step { id, tools, .. }) if *id == tv.step_id => {
                        tools.push(tv.clone())
                    }
                    _ => out.push(Block::Step {
                        id: tv.step_id.clone(),
                        ts: tv.ts.clone(),
                        t: tv.t.clone(),
                        tools: vec![tv.clone()],
                    }),
                },
                Event::ToolResult(rv) => {
                    'find: for b in out.iter_mut().rev() {
                        if let Block::Step { tools, .. } = b {
                            for t in tools.iter_mut() {
                                if t.id == rv.tool_id {
                                    t.result = Some(rv.clone());
                                    break 'find;
                                }
                            }
                        }
                    }
                }
                _ => {
                    if let Some(b) = self.block_at(i) {
                        out.push(b);
                    }
                }
            }
        }
        out
    }

    pub fn decisions(&self) -> Vec<&DecisionView> {
        self.events
            .iter()
            .filter_map(|e| {
                if let Event::Decision(d) = e {
                    Some(d)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn last_prompt_idx(&self) -> Option<usize> {
        self.events
            .iter()
            .rposition(|e| matches!(e, Event::Prompt { .. }))
    }

    pub fn prompt_id(&self, i: usize) -> Option<&str> {
        match &self.events[i] {
            Event::Prompt { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Stats for the work done in response to the prompt at event index `i`.
    pub fn prompt_stats(&self, i: usize) -> PromptStats {
        let end = self.events[i + 1..]
            .iter()
            .position(|e| matches!(e, Event::Prompt { .. }))
            .map(|p| i + 1 + p)
            .unwrap_or(self.events.len());
        let mut st = PromptStats::default();
        let mut files = BTreeSet::new();
        let mut last_ts = self.ts[i];
        for j in i + 1..end {
            if let Some(t) = self.ts[j] {
                last_ts = Some(t);
            }
            match &self.events[j] {
                Event::Tool(tv) => {
                    st.tools += 1;
                    if matches!(tv.name.as_str(), "Bash" | "exec_command" | "shell_command") {
                        st.commands += 1;
                    }
                    files.extend(tv.files.iter().cloned());
                }
                Event::ToolResult(r) if !r.ok => st.errors += 1,
                Event::Reasoning { .. } => st.reasoning += 1,
                Event::Decision(_) => st.decisions += 1,
                _ => {}
            }
        }
        st.files = files.into_iter().collect();
        if let (Some(a), Some(b)) = (self.ts[i], last_ts) {
            st.duration = fmt_duration((b - a).num_seconds());
        }
        st
    }

    pub fn session_stats(&self) -> SessionStats {
        let mut st = SessionStats::default();
        let mut files = BTreeSet::new();
        for e in &self.events {
            match e {
                Event::Prompt { .. } => st.prompts += 1,
                Event::Tool(tv) => {
                    st.tools += 1;
                    files.extend(tv.files.iter().cloned());
                }
                Event::ToolResult(r) if !r.ok => st.errors += 1,
                Event::Reasoning { .. } => st.reasoning += 1,
                Event::Decision(_) => st.decisions += 1,
                _ => {}
            }
        }
        st.files = files.len();
        let first = self.ts.iter().flatten().min().copied();
        let last = self.ts.iter().flatten().max().copied();
        if let (Some(a), Some(b)) = (first, last) {
            st.duration = fmt_duration((b - a).num_seconds());
            st.started = a.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string();
            st.last = fmt_t(Some(b));
        }
        st
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}
