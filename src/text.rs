//! Text utilities: prompt cleanup, markdown rendering, sentence heuristics.

use pulldown_cmark::{Event, Options, Parser, html};
use regex::Regex;
use std::sync::LazyLock;

static SYS_REMINDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").unwrap());
static IDE_SEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<ide_selection>.*?</ide_selection>").unwrap());
static PASTED_TAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?pasted_content[^>]*>").unwrap());
static CMD_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<command-name>(.*?)</command-name>").unwrap());
static CMD_ARGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<command-args>(.*?)</command-args>").unwrap());
static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*|__").unwrap());

/// Phrases that usually mark a choice being made in reasoning or narrative text.
static DECISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(i'?ll go with|going with|go with|i'?ll use|we'?ll use|let'?s use|let'?s go with|",
        r"i'?ll pick|i'?ll do|decided|decision|instead of|rather than|trade-?offs?|the right call|",
        r"the right default|settle[sd]? on|opt(?:ed|ing)? for|stick(?:ing)? with|recommend(?:ed|ation)?|",
        r"chose|choose|prefer(?:red)?|better fit|not worth|in favou?r of)\b",
    ))
    .unwrap()
});

/// Render markdown to HTML, escaping any raw HTML the source contains.
pub fn markdown(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(src, opts).map(|ev| match ev {
        Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
        other => other,
    });
    let mut out = String::with_capacity(src.len() * 2);
    html::push_html(&mut out, parser);
    out
}

/// Strip harness noise from a user message; `None` if nothing human remains.
pub fn clean_prompt(raw: &str) -> Option<String> {
    let s = SYS_REMINDER.replace_all(raw, "");
    let s = IDE_SEL.replace_all(&s, "");
    let s = PASTED_TAGS.replace_all(&s, "");
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    for noise in [
        "<local-command-stdout>",
        "<local-command-caveat>",
        "<bash-stdout>",
        "<bash-input>",
    ] {
        if t.starts_with(noise) {
            return None;
        }
    }
    if let Some(c) = CMD_NAME.captures(t) {
        let args = CMD_ARGS
            .captures(t)
            .map(|a| a[1].trim().to_string())
            .unwrap_or_default();
        return Some(format!("`{}` {}", c[1].trim(), args).trim().to_string());
    }
    Some(t.to_string())
}

pub fn first_line(s: &str) -> &str {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

pub fn first_sentence(text: &str, max: usize) -> String {
    let s = sentences(text).into_iter().next().unwrap_or("");
    truncate(&BOLD.replace_all(s, ""), max)
}

/// Split prose into sentences, skipping fenced code blocks and indented code.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.starts_with("    ") || line.starts_with('\t') {
            continue;
        }
        let mut start = 0;
        for (i, ch) in line.char_indices() {
            if matches!(ch, '.' | '!' | '?') {
                let next = line[i + 1..].chars().next();
                if next.is_none_or(|c| c.is_whitespace()) {
                    out.push(&line[start..=i]);
                    start = i + 1;
                }
            }
        }
        if start < line.len() {
            out.push(&line[start..]);
        }
    }
    out.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Sentences that read like a decision. Heuristic; callers label the result "inferred".
pub fn infer_decisions(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for s in sentences(text) {
        let n = s.chars().count();
        if !(30..=320).contains(&n) || s.starts_with('`') || s.starts_with('|') {
            continue;
        }
        if DECISION.is_match(s) {
            out.push(BOLD.replace_all(s, "").to_string());
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

pub fn fmt_duration(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

pub fn fmt_age(secs: u64) -> String {
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}
