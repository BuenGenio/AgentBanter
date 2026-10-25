//! Team membership comes only from recorded spawning relationships, never cwd.
use crate::{source::Source, store::SessionMeta};
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn members<'a>(metas: &'a [SessionMeta], selected: &str) -> Vec<&'a SessionMeta> {
    let mut ids = HashSet::from([selected.to_owned()]);
    loop {
        let before = ids.len();
        for m in metas {
            if let Some(parent) = &m.parent_id
                && (ids.contains(&m.id) || ids.contains(parent))
            {
                ids.insert(m.id.clone());
                ids.insert(parent.clone());
            }
        }
        if ids.len() == before {
            break;
        }
    }
    let mut out: Vec<_> = metas.iter().filter(|m| ids.contains(&m.id)).collect();
    out.sort_by_key(|m| (m.parent_id.is_some(), m.id.clone()));
    out
}

pub fn can_message(m: &SessionMeta) -> bool {
    // Child transcripts are not independent delivery endpoints. In particular,
    // Codex multi-agent v2 rejects direct app-server input to sub-agents.
    !matches!(m.source, Source::Chatgpt | Source::Ollama) && m.parent_id.is_none()
}

pub fn view(metas: &[SessionMeta], selected: &str) -> Value {
    let team = members(metas, selected);
    let members: Vec<_> = team.iter().map(|m| {
        let sendable = can_message(m);
        let parent_name = m.parent_id.as_ref().map(|id| metas.iter().find(|p| &p.id == id)
            .map(|p| p.agent_name.clone().unwrap_or_else(|| p.title.clone())).unwrap_or_else(|| format!("{id} (transcript unavailable)")));
        json!({
            "id":m.id, "parent_id":m.parent_id, "parent_name":parent_name,
            "name":m.agent_name.as_deref().unwrap_or(if m.parent_id.is_none() {m.agent.as_str()} else {m.title.as_str()}), "task":m.agent_task.as_ref().unwrap_or(&m.title), "provider":m.agent,
            "last_activity":m.mtime.to_rfc3339(), "cwd":m.cwd, "selected":m.id == selected,
            "recent_activity":m.live, "age":m.age, "sendable":sendable,
            "evidence":if m.parent_id.is_none() {"Selected conversation or recorded parent"} else if m.source == Source::Codex {"Codex thread_spawn parent_thread_id"} else if m.source==Source::Ollama {"Recorded Ollama trace-analysis relationship"} else {"Claude parent/subagents transcript location"},
            "mode":if !sendable {if m.source==Source::Ollama {"Continue this conversation through Local models."} else {"Direct messaging is unavailable for this child agent. Open or message its parent conversation."}}
                else if m.source == Source::Codex {"Queue to this Codex thread. Acceptance is not a reply."}
                else {"Continue Claude session; active sessions may fork. This does not message running children."}
        })
    }).collect();
    json!({"members":members,"note":if team.len() <= 1 {"No child-agent relationships recorded for this conversation yet."} else {"Recorded team relationships. Recent log activity does not prove an agent is still running."}})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meta(id: &str, parent: Option<&str>) -> SessionMeta {
        SessionMeta {
            id: id.into(),
            parent_id: parent.map(str::to_owned),
            agent_name: None,
            agent_task: None,
            source: Source::Codex,
            agent: "Codex".into(),
            path: Default::default(),
            slug: String::new(),
            cwd: "/same-project".into(),
            project: String::new(),
            title: id.into(),
            mtime: chrono::Utc::now(),
            age: String::new(),
            size: String::new(),
            live: false,
        }
    }
    #[test]
    fn family_includes_siblings_and_descendants_but_not_project_neighbors() {
        let all = vec![
            meta("root", None),
            meta("a", Some("root")),
            meta("b", Some("root")),
            meta("grandchild", Some("a")),
            meta("unrelated", None),
        ];
        let ids: Vec<_> = members(&all, "a").iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["root", "a", "b", "grandchild"]);
    }
    #[test]
    fn missing_parent_and_cycles_are_bounded() {
        let all = vec![meta("a", Some("missing")), meta("b", Some("missing"))];
        assert_eq!(members(&all, "a").len(), 2);
        let cycle = vec![meta("a", Some("b")), meta("b", Some("a"))];
        assert_eq!(members(&cycle, "a").len(), 2);
    }
    #[test]
    fn codex_children_cannot_receive_direct_input() {
        assert!(!can_message(&meta("child", Some("root"))));
        assert!(can_message(&meta("root", None)));
    }
    #[test]
    fn claude_children_are_visible_but_not_messageable() {
        let mut child = meta("child", Some("root"));
        child.source = Source::Claude;
        let v = view(&[meta("root", None), child], "root");
        assert_eq!(v["members"][1]["sendable"], false);
    }
}
