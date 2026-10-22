//! Import conversations.json from a ChatGPT data export. No app credentials or cloud APIs.
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::{collections::HashSet, fs, path::Path};

pub fn import(path: &Path, destination: &Path) -> Result<usize> {
    let export: Value = serde_json::from_slice(
        &fs::read(path).context("read ChatGPT export JSON (unzip the download first)")?,
    )?;
    let conversations = match &export {
        Value::Array(a) => a.clone(),
        Value::Object(_) if export["mapping"].is_object() => vec![export],
        _ => bail!(
            "expected conversations.json (an array of conversations, or one conversation with mapping)"
        ),
    };
    // Validate the whole input before changing the destination.
    let prepared = conversations
        .iter()
        .map(convert)
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir_all(destination)?;
    for (id, records) in &prepared {
        let mut body = String::new();
        for record in records {
            body.push_str(&serde_json::to_string(record)?);
            body.push('\n');
        }
        let target = destination.join(format!("{id}.jsonl"));
        if fs::read_to_string(&target).ok().as_deref() == Some(&body) {
            continue;
        }
        let temp = destination.join(format!(".{id}.{}.tmp", std::process::id()));
        fs::write(&temp, body)?;
        fs::rename(temp, target)?;
    }
    Ok(prepared.len())
}

fn convert(c: &Value) -> Result<(String, Vec<Value>)> {
    let id = c["id"]
        .as_str()
        .or_else(|| c["conversation_id"].as_str())
        .context("conversation has no id")?;
    ensure!(
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid conversation id"
    );
    let mapping = c["mapping"]
        .as_object()
        .context("conversation has no mapping")?;
    let leaf = c["current_node"]
        .as_str()
        .filter(|id| mapping.contains_key(*id))
        .or_else(|| {
            mapping
                .iter()
                .filter(|(_, n)| n["children"].as_array().is_none_or(Vec::is_empty))
                .max_by(|(ka, a), (kb, b)| {
                    a["message"]["create_time"]
                        .as_f64()
                        .unwrap_or(0.)
                        .total_cmp(&b["message"]["create_time"].as_f64().unwrap_or(0.))
                        .then(ka.cmp(kb))
                })
                .map(|(k, _)| k.as_str())
        });
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut node = leaf;
    while let Some(key) = node {
        ensure!(seen.insert(key), "cycle in conversation {id}");
        let value = mapping
            .get(key)
            .context("missing parent in ChatGPT conversation")?;
        chain.push((key, &value["message"]));
        node = value["parent"].as_str();
    }
    chain.reverse();
    let mut records = vec![json!({"cwd":"ChatGPT","type":"ai-title","aiTitle":c["title"]})];
    for (key, m) in chain {
        let role = m["author"]["role"].as_str().unwrap_or("");
        if !matches!(role, "user" | "assistant" | "tool")
            || m["metadata"]["is_visually_hidden_from_conversation"] == true
        {
            continue;
        }
        let content = &m["content"];
        let text = if let Some(parts) = content["parts"].as_array() {
            parts
                .iter()
                .map(|p| {
                    p.as_str().map(str::to_owned).unwrap_or_else(|| {
                        if p["content_type"] == "image_asset_pointer" {
                            "[Image attachment]".into()
                        } else {
                            "[Non-text attachment]".into()
                        }
                    })
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            content["text"].as_str().unwrap_or("").to_string()
        };
        if text.trim().is_empty() {
            continue;
        }
        let timestamp = m["create_time"]
            .as_f64()
            .and_then(|s| DateTime::<Utc>::from_timestamp_millis((s * 1000.) as i64))
            .map(|d| d.to_rfc3339());
        let block = if m["channel"] == "analysis" {
            json!({"type":"thinking","thinking":text})
        } else if role == "tool" {
            json!({"type":"text","text":format!("**Tool: {}**\n\n{text}",m["author"]["name"].as_str().unwrap_or("tool"))})
        } else {
            json!({"type":"text","text":text})
        };
        records.push(json!({"type":if role == "user" {"user"} else {"assistant"},"uuid":key,"timestamp":timestamp,"message":{"content":[block]}}));
    }
    Ok((id.to_owned(), records))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn conversation() -> Value {
        json!({"id":"chat-1","title":"Example","current_node":"b","mapping":{
            "r":{"parent":null,"message":null},
            "a":{"parent":"r","message":{"author":{"role":"user"},"content":{"parts":["Question"]},"create_time":1700000000}},
            "b":{"parent":"a","message":{"author":{"role":"assistant"},"content":{"parts":["Selected answer"]}}},
            "other":{"parent":"a","message":{"author":{"role":"assistant"},"content":{"parts":["Old answer"]}}}
        }})
    }
    #[test]
    fn active_branch_only() {
        let (_, r) = convert(&conversation()).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[2]["message"]["content"][0]["text"], "Selected answer");
        assert!(r[1]["timestamp"].as_str().unwrap().starts_with("2023-"));
    }
    #[test]
    fn rejects_path_traversal_and_cycles() {
        let mut c = conversation();
        c["id"] = json!("../escape");
        assert!(convert(&c).is_err());
        c["id"] = json!("valid");
        c["mapping"]["a"]["parent"] = json!("b");
        assert!(convert(&c).is_err());
    }
}
