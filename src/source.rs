//! Translate source-specific records into the timeline's common transcript format.
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
    Chatgpt,
    Ollama,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Chatgpt => "ChatGPT",
            Self::Ollama => "Ollama",
        }
    }
    pub fn session_id(self, native: &str) -> String {
        match self {
            Self::Claude => native.to_owned(),
            Self::Codex => format!("codex--{native}"),
            Self::Chatgpt => format!("chatgpt--{native}"),
            Self::Ollama => format!("ollama--{native}"),
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
        "event_msg" if p["type"] == "task_started" => {
            vec![json!({"type":"turn_start", "turn_id":p["turn_id"]})]
        }
        "event_msg" if p["type"] == "task_complete" => {
            vec![
                json!({"type":"turn_metrics", "turn_id":p["turn_id"], "timestamp":ts,
                "metrics":{"duration_ms":p["duration_ms"], "first_token_ms":p["time_to_first_token_ms"]}}),
            ]
        }
        "event_msg" if p["type"] == "item_completed" && p["item"]["type"] == "AgentMessage" => {
            let duration = p["completed_at_ms"]
                .as_u64()
                .zip(p["started_at_ms"].as_u64())
                .and_then(|(end, start)| end.checked_sub(start));
            vec![
                json!({"type":"turn_metrics", "turn_id":p["turn_id"], "timestamp":ts,
                "metrics":{"message_ms":duration}}),
            ]
        }
        "token_usage_record" => {
            vec![
                json!({"type":"turn_metrics", "turn_id":p["turn_id"], "timestamp":ts,
                "metrics":{"usage":p["turn_token_usage"]}}),
            ]
        }
        // Remaining event_msg records mirror messages and tools.
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
    fn object_arguments_remain_structured() {
        let event = json!({"type":"response_item","payload":{"type":"function_call","call_id":"call","name":"exec_command","arguments":{"cmd":"pwd"}}});
        let result = normalize(Source::Codex, &event, "id");
        assert_eq!(result[0]["message"]["content"][0]["input"]["cmd"], "pwd");
    }
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
    fn turn_metrics_keep_identity_and_do_not_duplicate_messages() {
        let mut session = Session::new("metrics");
        for v in [
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"a"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"First"}]}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"a","item":{"type":"AgentMessage"},"started_at_ms":1000,"completed_at_ms":4631}}),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"b"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Second"}]}}),
            json!({"type":"token_usage_record","payload":{"turn_id":"a","turn_token_usage":{"input_tokens":34766,"cached_input_tokens":34432,"output_tokens":122,"reasoning_output_tokens":0}}}),
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"a","duration_ms":4684,"time_to_first_token_ms":1000}}),
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"unknown","duration_ms":999}}),
        ] {
            for r in normalize(Source::Codex, &v, "fixture") {
                session.ingest(&r);
            }
        }
        let first = session.prompt_stats(0);
        assert_eq!(first.duration, "4.684 s");
        assert_eq!(first.activity.first_token.as_deref(), Some("1.000 s"));
        assert_eq!(first.activity.messages.as_deref(), Some("3.631 s"));
        assert_eq!(
            first.activity.usage.as_ref().unwrap()["reasoning_output_tokens"],
            0
        );
        assert!(
            !session
                .prompt_stats(session.last_prompt_idx().unwrap())
                .activity
                .available
        );
        assert_eq!(session.blocks().len(), 2);
        let renderer = crate::render::Renderer::new(None);
        let html = renderer
            .render(
                "partials/chips.html",
                minijinja::context! { stats => first },
            )
            .unwrap();
        assert!(html.contains("3.631 s"));
        assert!(html.contains("Reasoning tokens</dt><dd>0"));
    }

    #[test]
    fn injected_context_is_hidden() {
        let v = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>private</environment_context>"}]}});
        assert!(normalize(Source::Codex, &v, "1").is_empty());
    }
}
