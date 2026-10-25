//! Sends a follow-up prompt to a session by running `claude -p --resume <id>` in the session's directory.
//! The reply lands in a transcript, so the tailer picks it up like any other activity.
//!
//! A session still held by another `claude` process (the VS Code / Cursor extension keeps one per open
//! chat) must not be appended to from here: both processes would write turns onto the same parent and the
//! conversation would split. Those sessions are continued with `--fork-session`, which starts a new
//! transcript carrying the full history.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[derive(Clone, Serialize, Debug)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SendStatus {
    Idle,
    Queued {
        session: String,
    },
    /// `session` is where the reply is being written: the same id, or the fork's once known.
    Running {
        session: Option<String>,
    },
    Done {
        session: Option<String>,
    },
    Failed {
        error: String,
    },
}

pub struct Sender {
    pub claude_bin: PathBuf,
    pub codex_bin: PathBuf,
    pub permission_mode: String,
    status: Mutex<HashMap<String, SendStatus>>,
}

impl Sender {
    pub fn new(claude_bin: PathBuf, codex_bin: PathBuf, permission_mode: String) -> Arc<Self> {
        Arc::new(Self {
            claude_bin,
            codex_bin,
            permission_mode,
            status: Mutex::new(HashMap::new()),
        })
    }

    pub fn status(&self, id: &str) -> SendStatus {
        self.status
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or(SendStatus::Idle)
    }

    pub fn queue_codex(
        self: &Arc<Self>,
        id: &str,
        cwd: PathBuf,
        text: String,
    ) -> anyhow::Result<()> {
        {
            let mut status = self.status.lock().unwrap();
            if matches!(status.get(id), Some(SendStatus::Running { .. })) {
                anyhow::bail!("a message to this recipient is still being submitted");
            }
            status.insert(
                id.into(),
                SendStatus::Running {
                    session: Some(id.into()),
                },
            );
        }
        let me = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let native = id.strip_prefix("codex--").unwrap_or(&id);
            let mut cmd = Command::new(&me.codex_bin);
            cmd.args(["queue", "--thread", native, "--message", &text])
                .current_dir(cwd)
                .kill_on_drop(true);
            let status = match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
                Ok(Ok(out)) if out.status.success() => SendStatus::Queued { session: id.clone() },
                Ok(Ok(out)) => SendStatus::Failed { error: format!("Codex did not confirm queuing: {}", String::from_utf8_lossy(&out.stderr).chars().take(1000).collect::<String>()) },
                Ok(Err(e)) => SendStatus::Failed { error: format!("Cannot run Codex: {e}") },
                Err(_) => SendStatus::Failed { error: "Queue confirmation timed out. Delivery is uncertain; check the target conversation before retrying.".into() },
            };
            me.status.lock().unwrap().insert(id, status);
        });
        Ok(())
    }

    /// Start `claude` for this session unless one we started is still running.
    pub fn start(
        self: &Arc<Self>,
        id: &str,
        cwd: PathBuf,
        text: String,
        fork: bool,
    ) -> anyhow::Result<()> {
        {
            let mut status = self.status.lock().unwrap();
            if matches!(status.get(id), Some(SendStatus::Running { .. })) {
                anyhow::bail!("a message to this session is still being processed");
            }
            let session = (!fork).then(|| id.to_string());
            status.insert(id.to_string(), SendStatus::Running { session });
        }
        let me = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let done = match me.run(&id, &cwd, &text, fork).await {
                Ok(session) => SendStatus::Done { session },
                Err(e) => SendStatus::Failed {
                    error: format!("{e:#}"),
                },
            };
            me.status.lock().unwrap().insert(id, done);
        });
        Ok(())
    }

    /// Runs one turn; returns the id of the session the reply was written to.
    async fn run(
        &self,
        id: &str,
        cwd: &Path,
        text: &str,
        fork: bool,
    ) -> anyhow::Result<Option<String>> {
        let mut cmd = Command::new(&self.claude_bin);
        cmd.args([
            "-p",
            "--resume",
            id,
            "--permission-mode",
            &self.permission_mode,
        ])
        .args(["--output-format", "stream-json", "--verbose"]);
        if fork {
            cmd.arg("--fork-session");
        }
        // The prompt goes in on stdin so it is never parsed as flags.
        let mut child = cmd
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot run {}: {e}", self.claude_bin.display()))?;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);

        let mut stderr = child.stderr.take().unwrap();
        let err_task = tokio::spawn(async move {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s).await;
            s
        });
        // Every stream-json record carries session_id; publish the first so the page can follow a fork.
        let mut session = None;
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        while let Some(line) = lines.next_line().await? {
            if session.is_none()
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                && let Some(sid) = v["session_id"].as_str()
            {
                session = Some(sid.to_string());
                self.status.lock().unwrap().insert(
                    id.to_string(),
                    SendStatus::Running {
                        session: session.clone(),
                    },
                );
            }
        }
        let status = child.wait().await?;
        if !status.success() {
            let err = err_task.await.unwrap_or_default();
            let lines: Vec<&str> = err.trim().lines().collect();
            let tail = lines[lines.len().saturating_sub(5)..].join("\n");
            anyhow::bail!("claude exited with {status}: {tail}");
        }
        Ok(session)
    }
}

/// Whether some other running process has this session open, judged from `/proc/*/cmdline`
/// (`--resume <id>`, `--resume=<id>`, `--session-id <id>`). Always false off Linux.
pub fn held_elsewhere(id: &str) -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return false;
    };
    let me = std::process::id().to_string();
    procs.flatten().any(|p| {
        let name = p.file_name();
        let name = name.to_string_lossy();
        if name == me || !name.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let Ok(raw) = std::fs::read(p.path().join("cmdline")) else {
            return false;
        };
        let args: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
        args.iter().enumerate().any(|(i, a)| {
            let next_is_id = || args.get(i + 1).is_some_and(|n| *n == id.as_bytes());
            match *a {
                b"--resume" | b"-r" | b"--session-id" => next_is_id(),
                a => [&b"--resume="[..], b"--session-id="]
                    .iter()
                    .any(|p| a.strip_prefix(*p) == Some(id.as_bytes())),
            }
        })
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[tokio::test]
    async fn codex_queue_preserves_message_and_reports_acceptance() {
        let dir =
            std::env::temp_dir().join(format!("agentbanter-send-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("codex-mock");
        std::fs::write(&script, "#!/bin/sh\n[ \"$1\" = queue ] && [ \"$2\" = --thread ] && [ \"$3\" = thread-123 ] && [ \"$4\" = --message ] || exit 7\nprintf '%s' \"$5\" > message.txt\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sender = Sender::new("unused".into(), script, "auto".into());
        let text = "Question with 'quotes'\nand $(literal text)";
        sender
            .queue_codex("codex--thread-123", dir.clone(), text.into())
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if !matches!(
                    sender.status("codex--thread-123"),
                    SendStatus::Running { .. }
                ) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            sender.status("codex--thread-123"),
            SendStatus::Queued { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(dir.join("message.txt")).unwrap(),
            text
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
