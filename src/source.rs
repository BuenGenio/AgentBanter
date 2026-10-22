//! Translate source-specific records into the timeline's common transcript format.
use clap::ValueEnum;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
    Chatgpt,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Chatgpt => "ChatGPT",
        }
    }
    pub fn session_id(self, native: &str) -> String {
        match self {
            Self::Claude => native.to_owned(),
            Self::Codex => format!("codex--{native}"),
            Self::Chatgpt => format!("chatgpt--{native}"),
        }
    }
}

pub fn normalize(source: Source, v: &Value, fallback_id: &str) -> Vec<Value> {
    if source != Source::Codex {
        return vec![v.clone()];
    }
    let p = &v["payload"];
    let ts = &v["timestamp"];
    let id = p["id"].as_str().unwrap_or(fallback_id);
    let record = |role: &str, content: Value| {
        json!({
            "type": role, "uuid": id, "timestamp": ts, "message": {"content": content}
        })
    };
    match v["type"].as_str().unwrap_or("") {
        "session_meta" | "turn_context" => {
            vec![json!({"cwd":p["cwd"], "gitBranch":p["git"]["branch"]})]
        }
        "response_item" => match p["type"].as_str().unwrap_or("") {
            "message" => {
                let role = p["role"].as_str().unwrap_or("");
                if !matches!(role, "user" | "assistant") {
                    return vec![];
                }
                let text = p["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| match c["type"].as_str().unwrap_or("") {
                        "input_text" | "output_text" | "text" => c["text"].as_str(),
                        "input_image" | "image" => Some("[Image attachment]"),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                // User-role records also carry injected app/repository context.
                if role == "user" && is_context(&text) {
                    return vec![];
                }
                let content = if p["channel"] == "analysis" {
                    json!([{"type":"thinking", "thinking":text}])
                } else {
                    json!([{"type":"text", "text":text}])
                };
                vec![record(role, content)]
            }
            "reasoning" => {
                // Only plaintext summaries are available; encrypted reasoning is never decoded.
                let text = p["summary"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                vec![record(
                    "assistant",
                    json!([{"type":"thinking", "thinking":text}]),
                )]
            }
            "function_call" | "custom_tool_call" => {
                let name = p["name"].as_str().unwrap_or("tool");
                let raw = p
                    .get("arguments")
                    .or_else(|| p.get("input"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let input = if raw.is_object() {
                    raw.clone()
                } else {
                    raw.as_str()
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .unwrap_or_else(|| {
                            if name == "apply_patch" {
                                json!({"patch":raw.as_str().unwrap_or("")})
                            } else {
                                json!({"input":raw})
                            }
                        })
                };
                vec![record(
                    "assistant",
                    json!([{"type":"tool_use", "id":p["call_id"], "name":name, "input":input}]),
                )]
            }
            "function_call_output" | "custom_tool_call_output" => {
                let output = p["output"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| p["output"].to_string());
                let parsed = serde_json::from_str::<Value>(&output).ok();
                let failed = parsed.as_ref().is_some_and(|o| {
                    o["isError"] == true || o["exit_code"].as_i64().is_some_and(|n| n != 0)
                }) || output.lines().any(|l| {
                    l.strip_prefix("Process exited with code ")
                        .is_some_and(|c| c.trim() != "0")
                });
                vec![record(
                    "user",
                    json!([{"type":"tool_result", "tool_use_id":p["call_id"], "is_error":failed, "content":output}]),
                )]
            }
            _ => vec![],
        },
        // event_msg mirrors response_item; consuming both would duplicate messages and tools.
        _ => vec![],
    }
}

fn is_context(text: &str) -> bool {
    let text = text.trim_start();
    [
        "<environment_context>",
        "<permissions instructions>",
        "# AGENTS.md instructions",
        "<INSTRUCTIONS>",
        "<external_codex_apps_open_page>",
    ]
    .iter()
    .any(|p| text.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Session};
    #[test]
    fn codex_messages_tools_and_results_without_duplicates() {
        let mut session = Session::new("test");
        for v in [
            json!({"type":"session_meta","payload":{"cwd":"/project","git":{"branch":"main"}}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Fix it"}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Fix it"}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"c1","name":"exec_command","arguments":"{\"cmd\":\"false\"}"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"Process exited with code 1"}}),
            json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"Checking the failure."}],"encrypted_content":"secret"}}),
        ] {
            for r in normalize(Source::Codex, &v, &session.events.len().to_string()) {
                session.ingest(&r);
            }
        }
        assert_eq!(session.cwd.as_deref(), Some("/project"));
        assert_eq!(
            session
                .events
                .iter()
                .filter(|e| matches!(e, Event::Prompt { .. }))
                .count(),
            1
        );
        assert!(
            session
                .events
                .iter()
                .any(|e| matches!(e,Event::ToolResult(r) if !r.ok && r.tool_id == "c1"))
        );
        assert!(session.events.iter().any(|e|matches!(e,Event::Reasoning{html,..} if html.contains("Checking") && !html.contains("secret"))));
    }
    #[test]
    fn injected_context_is_hidden() {
        let v = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>private</environment_context>"}]}});
        assert!(normalize(Source::Codex, &v, "1").is_empty());
    }
}
