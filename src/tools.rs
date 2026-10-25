//! Turn a `tool_use` block into a one-line summary plus expandable detail.

use crate::text::{first_line, truncate};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Targets of `>`/`>>`/`tee` in a shell command, so files written via heredocs count as touched.
/// The redirect must not be part of an arrow (`->`, `=>`, `<>`), and heredoc bodies are skipped first.
static SHELL_TARGET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[^-=<>\w])>>?\s*['"]?([\w~./-]+)|\btee(?:\s+-a)?\s+['"]?([\w~./-]+)"#)
        .unwrap()
});
static HEREDOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<<-?\s*['"]?(\w+)['"]?"#).unwrap());

pub fn shell_targets(cmd: &str) -> Vec<String> {
    let cmd = strip_heredocs(cmd);
    let mut out: Vec<String> = SHELL_TARGET
        .captures_iter(&cmd)
        .filter_map(|c| c.get(1).or(c.get(2)).map(|m| m.as_str().to_string()))
        .filter(|t| {
            !t.starts_with("/dev/")
                && !t.starts_with("/tmp/")
                && !t.starts_with("/proc/")
                && (t.contains('/') || t.contains('.'))
        })
        .collect();
    out.dedup();
    out
}

/// Remove heredoc bodies (`<<EOF ... EOF`) so their contents aren't mistaken for shell syntax.
fn strip_heredocs(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut rest = cmd;
    while let Some(m) = HEREDOC.captures(rest) {
        let delim = m[1].to_string();
        let marker_end = m.get(0).unwrap().end();
        let eol = rest[marker_end..]
            .find('\n')
            .map(|p| marker_end + p)
            .unwrap_or(rest.len());
        out.push_str(&rest[..eol]);
        let body = &rest[eol..];
        let mut consumed = body.len();
        let mut pos = 0;
        for line in body.split_inclusive('\n') {
            pos += line.len();
            if line.trim() == delim {
                consumed = pos;
                break;
            }
        }
        rest = &body[consumed..];
    }
    out.push_str(rest);
    out
}

pub struct ToolSummary {
    /// CSS class used for the marker colour: bash, edit, write, read, search, ask, agent, web, plan, tool.
    pub icon: &'static str,
    pub summary: String,
    pub detail: String,
    /// Files this call wrote, relative to the project when possible.
    pub files: Vec<String>,
}

pub fn summarize(name: &str, input: &Value, cwd: Option<&str>) -> ToolSummary {
    let s = |k: &str| input[k].as_str().unwrap_or("");
    let rel = |p: &str| -> String {
        match cwd {
            Some(c) if p.starts_with(c) => p[c.len()..].trim_start_matches('/').to_string(),
            _ => p.to_string(),
        }
    };
    let pretty = || {
        truncate(
            &serde_json::to_string_pretty(input).unwrap_or_default(),
            2000,
        )
    };
    let mut out = ToolSummary {
        icon: "tool",
        summary: name.to_string(),
        detail: String::new(),
        files: Vec::new(),
    };

    match name {
        "Bash" | "exec_command" | "shell_command" => {
            let cmd = input["cmd"].as_str().unwrap_or_else(|| s("command"));
            let desc = s("description");
            out.icon = "bash";
            out.summary = truncate(
                if desc.is_empty() {
                    first_line(cmd)
                } else {
                    desc
                },
                140,
            );
            out.detail = truncate(cmd, 4000);
            out.files = shell_targets(cmd).into_iter().map(|t| rel(&t)).collect();
        }
        "apply_patch" => {
            let patch = s("patch");
            out.icon = "edit";
            out.files = patch
                .lines()
                .filter_map(|l| {
                    [
                        "*** Add File: ",
                        "*** Update File: ",
                        "*** Delete File: ",
                        "*** Move to: ",
                    ]
                    .iter()
                    .find_map(|prefix| l.strip_prefix(prefix))
                })
                .map(rel)
                .collect();
            out.summary = truncate(&format!("Patch {}", out.files.join(", ")), 140);
            out.detail = truncate(patch, 4000);
        }
        "exec" => {
            out.icon = "tool";
            out.summary = "Run tools".to_string();
            out.detail = s("input").to_owned();
        }
        "Edit" | "MultiEdit" => {
            let p = rel(s("file_path"));
            out.icon = "edit";
            out.summary = format!("Edit {p}");
            out.detail = truncate(
                &format!("--- old\n{}\n+++ new\n{}", s("old_string"), s("new_string")),
                4000,
            );
            out.files = vec![p];
        }
        "Write" => {
            let p = rel(s("file_path"));
            out.icon = "write";
            out.summary = format!("Write {p}");
            out.detail = truncate(s("content"), 4000);
            out.files = vec![p];
        }
        "NotebookEdit" => {
            let p = rel(s("notebook_path"));
            out.icon = "edit";
            out.summary = format!("Edit notebook {p}");
            out.detail = truncate(s("new_source"), 4000);
            out.files = vec![p];
        }
        "Read" => {
            let p = rel(s("file_path"));
            out.icon = "read";
            out.summary = format!("Read {p}");
        }
        "Glob" => {
            out.icon = "search";
            out.summary = format!("Glob {}", s("pattern"));
        }
        "Grep" => {
            out.icon = "search";
            out.summary = truncate(&format!("Grep {}", s("pattern")), 140);
            out.detail = rel(s("path"));
        }
        "AskUserQuestion" => {
            out.icon = "ask";
            let qs: Vec<&Value> = input["questions"]
                .as_array()
                .map(|a| a.iter().collect())
                .unwrap_or_default();
            let titles: Vec<&str> = qs.iter().filter_map(|q| q["question"].as_str()).collect();
            out.summary = truncate(&format!("Asked: {}", titles.join(" · ")), 160);
            let mut d = String::new();
            for q in qs {
                d.push_str(q["question"].as_str().unwrap_or(""));
                d.push('\n');
                for o in q["options"].as_array().into_iter().flatten() {
                    d.push_str(&format!(
                        "  • {} — {}\n",
                        o["label"].as_str().unwrap_or(""),
                        o["description"].as_str().unwrap_or("")
                    ));
                }
            }
            out.detail = truncate(&d, 4000);
        }
        "Agent" | "Task" => {
            out.icon = "agent";
            let kind = s("subagent_type");
            out.summary = truncate(
                &format!(
                    "Agent{}: {}",
                    if kind.is_empty() {
                        String::new()
                    } else {
                        format!(" ({kind})")
                    },
                    s("description")
                ),
                140,
            );
            out.detail = truncate(s("prompt"), 3000);
        }
        "WebFetch" | "WebSearch" => {
            out.icon = "web";
            let target = if s("url").is_empty() {
                s("query")
            } else {
                s("url")
            };
            out.summary = truncate(&format!("{name} {target}"), 140);
        }
        "Skill" => {
            out.icon = "plan";
            out.summary = truncate(&format!("Skill /{} {}", s("skill"), s("args")), 140);
        }
        "TodoWrite" => {
            out.icon = "plan";
            out.summary = "Update todo list".into();
            out.detail = pretty();
        }
        "EnterPlanMode" | "ExitPlanMode" => {
            out.icon = "plan";
            out.summary = name.into();
            out.detail = truncate(s("plan"), 4000);
        }
        "Artifact" => {
            out.icon = "write";
            let action = if s("action").is_empty() {
                "publish"
            } else {
                s("action")
            };
            out.summary = truncate(&format!("Artifact {action} {}", rel(s("file_path"))), 140);
        }
        _ => {
            out.detail = pretty();
        }
    }
    out
}

/// Unwrap text-content and command-output envelopes without interpreting arbitrary code.
pub fn readable_output(text: &str) -> String {
    fn unpack(value: &Value, depth: usize) -> String {
        if depth > 8 {
            return value.to_string();
        }
        match value {
            Value::String(text) => serde_json::from_str::<Value>(text)
                .map(|v| unpack(&v, depth + 1))
                .unwrap_or_else(|_| text.clone()),
            Value::Array(items) if items.iter().all(|v| v.get("text").is_some()) => items
                .iter()
                .map(|v| unpack(&v["text"], depth + 1))
                .collect::<Vec<_>>()
                .join("\n\n"),
            Value::Object(object) if object.contains_key("output") => {
                let mut meta = Vec::new();
                if let Some(exit) = object.get("exit_code").filter(|v| !v.is_null()) {
                    meta.push(format!("Exit {exit}"));
                }
                if let Some(time) = object.get("wall_time_seconds") {
                    meta.push(format!("{time} s"));
                }
                format!(
                    "{}\n{}",
                    meta.join(" · "),
                    unpack(&object["output"], depth + 1)
                )
                .trim()
                .to_owned()
            }
            _ => serde_json::to_string_pretty(value).unwrap_or_default(),
        }
    }
    serde_json::from_str::<Value>(text)
        .map(|v| unpack(&v, 0))
        .unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod output_tests {
    use super::*;
    #[test]
    fn nested_output_is_readable_and_original_is_preserved_in_view() {
        let raw = serde_json::json!([{"type":"input_text","text":"Script completed"},{"type":"input_text","text":"{\"exit_code\":0,\"output\":\"hello\\nworld\\n\"}"}]).to_string();
        let readable = readable_output(&raw);
        assert!(readable.contains("hello\nworld"));
        assert!(!readable.contains("input_text"));
        let r = crate::model::ResultView {
            raw,
            tool_id: "x".into(),
            t: String::new(),
            ok: true,
            summary: "done".into(),
            detail: readable,
        };
        let tool = crate::model::ToolView {
            id: "x".into(),
            step_id: "x".into(),
            ts: String::new(),
            t: String::new(),
            name: "exec".into(),
            icon: "tool".into(),
            summary: "Run".into(),
            detail: "</script><script>alert(1)</script>".into(),
            files: vec![],
            result: Some(r),
        };
        let html = crate::render::Renderer::new(None)
            .render("partials/tool.html", minijinja::context! { tool })
            .unwrap();
        assert!(html.contains("inspector-data"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_but_not_arrows_or_heredoc_bodies() {
        let cmd = "mkdir -p docs && cat > docs/ADR-001.md <<'EOF'\n# Title\nfn f() -> Vec<u8> { x => y }\ncat > not/this.md\nEOF\necho hi >> notes/log.md 2>/dev/null; cargo build 2>&1 | tee build.log";
        assert_eq!(
            shell_targets(cmd),
            vec!["docs/ADR-001.md", "notes/log.md", "build.log"]
        );
    }

    #[test]
    fn ignores_plain_words_and_devices() {
        assert!(shell_targets("ls -la > /dev/null; a -> b; x => y; echo > out").is_empty());
    }
}
