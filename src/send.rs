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
    /// `session` is where the reply is being written: the same id, or the fork's once known.
    Running { session: Option<String> },
    Done { session: Option<String> },
    Failed { error: String },
}

pub struct Sender {
    pub claude_bin: PathBuf,
    pub permission_mode: String,
    status: Mutex<HashMap<String, SendStatus>>,
}

impl Sender {
    pub fn new(claude_bin: PathBuf, permission_mode: String) -> Arc<Self> {
        Arc::new(Self {
            claude_bin,
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

    /// Start `claude` for this session unless one we started is still running.
    pub fn start(self: &Arc<Self>, id: &str, cwd: PathBuf, text: String, fork: bool) -> anyhow::Result<()> {
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
    async fn run(&self, id: &str, cwd: &Path, text: &str, fork: bool) -> anyhow::Result<Option<String>> {
        let mut cmd = Command::new(&self.claude_bin);
        cmd.args(["-p", "--resume", id, "--permission-mode", &self.permission_mode])
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
