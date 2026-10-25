//! Portable CLI startup, explicit configuration and local server discovery.
use crate::Args;
use anyhow::{Context, Result, bail, ensure};
use clap::{ArgMatches, parser::ValueSource};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub fn config_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("cannot locate user config directory")?
        .join("agentbanter/config.json"))
}
pub fn configure(args: &mut Args, matches: &ArgMatches) -> Result<()> {
    let path = args.config.clone().map(Ok).unwrap_or_else(config_path)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && args.config.is_none() => {
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("read configuration {}", path.display())),
    };
    let data: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let object = data
        .as_object()
        .context("configuration must be a JSON object")?;
    const KEYS: &[&str] = &[
        "port",
        "bind",
        "sources",
        "projects_dir",
        "codex_dir",
        "chatgpt_dir",
        "ollama_dir",
        "ollama_url",
        "allow_ollama",
        "poll_ms",
        "allow_capture",
        "allow_send",
        "codex_bin",
        "claude_bin",
        "send_permission_mode",
        "templates",
        "static_dir",
    ];
    for key in object.keys() {
        ensure!(
            KEYS.contains(&key.as_str()),
            "unknown configuration key: {key}"
        );
    }
    macro_rules! setting {
        ($field:ident, $kind:ty) => {
            if matches.value_source(stringify!($field)) != Some(ValueSource::CommandLine) {
                if let Some(value) = object.get(stringify!($field)) {
                    args.$field = serde_json::from_value::<$kind>(value.clone())
                        .with_context(|| format!("invalid setting {}", stringify!($field)))?;
                }
            }
        };
    }
    setting!(port, u16);
    setting!(bind, Vec<IpAddr>);
    setting!(sources, Vec<crate::source::Source>);
    setting!(poll_ms, u64);
    setting!(allow_capture, bool);
    setting!(allow_send, bool);
    setting!(allow_ollama, bool);
    setting!(ollama_url, String);
    setting!(ollama_dir, Option<PathBuf>);
    setting!(codex_bin, PathBuf);
    setting!(claude_bin, PathBuf);
    setting!(send_permission_mode, String);
    setting!(projects_dir, Option<PathBuf>);
    setting!(codex_dir, Option<PathBuf>);
    setting!(chatgpt_dir, Option<PathBuf>);
    setting!(templates, Option<PathBuf>);
    // clap's public flag name is --static, but its field identifier remains static_dir.
    setting!(static_dir, Option<PathBuf>);
    let base = path.parent().unwrap_or(Path::new("."));
    for (id, path) in [
        ("projects_dir", &mut args.projects_dir),
        ("codex_dir", &mut args.codex_dir),
        ("chatgpt_dir", &mut args.chatgpt_dir),
        ("ollama_dir", &mut args.ollama_dir),
        ("templates", &mut args.templates),
        ("static_dir", &mut args.static_dir),
    ] {
        if matches.value_source(id) != Some(ValueSource::CommandLine)
            && let Some(path) = path
            && path.is_relative()
        {
            *path = std::env::current_dir()?.join(base).join(&*path);
        }
    }
    for (id, executable) in [
        ("codex_bin", &mut args.codex_bin),
        ("claude_bin", &mut args.claude_bin),
    ] {
        if matches.value_source(id) != Some(ValueSource::CommandLine)
            && executable.is_relative()
            && executable.components().count() > 1
        {
            *executable = std::env::current_dir()?.join(base).join(&*executable);
        }
    }
    ensure!(!args.sources.is_empty(), "sources must not be empty");
    ensure!(args.poll_ms >= 50, "poll_ms must be at least 50");
    Ok(())
}

pub fn init_config(explicit: Option<&Path>) -> Result<()> {
    let path = explicit
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(config_path)?;
    fs::create_dir_all(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("create {} (existing files are preserved)", path.display()))?;
    use std::io::Write;
    writeln!(
        file,
        "{}",
        serde_json::to_string_pretty(
            &json!({"bind":["127.0.0.1"],"port":7331,"allow_capture":false,"allow_send":false,"allow_ollama":false,"sources":["claude","codex","chatgpt","ollama"],"poll_ms":750})
        )?
    )?;
    println!("Created {}", path.display());
    Ok(())
}

pub async fn browser(url: &str) -> Result<()> {
    let mut command;
    #[cfg(target_os = "macos")]
    {
        command = tokio::process::Command::new("open");
        command.arg(url);
    }
    #[cfg(target_os = "linux")]
    {
        command = tokio::process::Command::new("xdg-open");
        command.arg(url);
    }
    #[cfg(target_os = "windows")]
    {
        command = tokio::process::Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", url]);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("open {url} in your browser");
    }
    let status = tokio::time::timeout(Duration::from_secs(10), command.status())
        .await
        .context("browser launcher timed out")??;
    ensure!(status.success(), "browser launcher exited with {status}");
    Ok(())
}

/// Identify our server rather than treating an arbitrary occupied port as success.
pub async fn healthy(addr: SocketAddr) -> Result<bool> {
    let result = tokio::time::timeout(Duration::from_millis(750), async {
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        stream
            .write_all(
                format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut bytes = Vec::new();
        stream.take(16384).read_to_end(&mut bytes).await?;
        let text = String::from_utf8(bytes)?;
        let (headers, body) = text
            .split_once("\r\n\r\n")
            .context("invalid HTTP health response")?;
        ensure!(
            headers
                .lines()
                .next()
                .is_some_and(|s| s.starts_with("HTTP/1.1 200 ") || s.starts_with("HTTP/1.0 200 ")),
            "port {addr} is occupied by a different server"
        );
        let health: Value = serde_json::from_str(body)
            .context("port occupied: not an AgentBanter health response")?;
        ensure!(
            health["app"] == "agentbanter",
            "port {addr} is occupied by a different application"
        );
        Ok::<_, anyhow::Error>(true)
    })
    .await;
    match result {
        Ok(Ok(yes)) => Ok(yes),
        Ok(Err(error))
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused) =>
        {
            Ok(false)
        }
        Ok(Err(error)) => Err(error),
        Err(_) => bail!("server at {addr} did not answer its health check"),
    }
}

fn server_args(args: &Args) -> Vec<OsString> {
    let mut result: Vec<OsString> = vec![
        "serve".into(),
        "--port".into(),
        args.port.to_string().into(),
        "--poll-ms".into(),
        args.poll_ms.to_string().into(),
    ];
    for ip in if args.bind.is_empty() {
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
    } else {
        args.bind.clone()
    } {
        result.extend(["--bind".into(), ip.to_string().into()]);
    }
    let sources = args
        .sources
        .iter()
        .map(|s| match s {
            crate::source::Source::Claude => "claude",
            crate::source::Source::Codex => "codex",
            crate::source::Source::Chatgpt => "chatgpt",
            crate::source::Source::Ollama => "ollama",
        })
        .collect::<Vec<_>>()
        .join(",");
    result.extend(["--sources".into(), sources.into()]);
    for (flag, path) in [
        ("--projects-dir", &args.projects_dir),
        ("--codex-dir", &args.codex_dir),
        ("--chatgpt-dir", &args.chatgpt_dir),
        ("--ollama-dir", &args.ollama_dir),
        ("--templates", &args.templates),
        ("--static", &args.static_dir),
    ] {
        if let Some(path) = path {
            result.extend([flag.into(), path.as_os_str().to_owned()]);
        }
    }
    for (flag, path) in [
        ("--codex-bin", &args.codex_bin),
        ("--claude-bin", &args.claude_bin),
    ] {
        result.extend([flag.into(), path.as_os_str().to_owned()]);
    }
    result.extend([
        "--send-permission-mode".into(),
        args.send_permission_mode.clone().into(),
    ]);
    result.extend(["--ollama-url".into(), args.ollama_url.clone().into()]);
    if args.allow_ollama {
        result.push("--allow-ollama".into());
    }
    if args.allow_capture {
        result.push("--allow-capture".into());
    }
    if args.allow_send {
        result.push("--allow-send".into());
    }
    if let Some(path) = &args.config {
        result.extend(["--config".into(), path.as_os_str().to_owned()]);
    }
    result
}

pub async fn open(args: &Args) -> Result<()> {
    ensure!(args.port != 0, "open needs a fixed nonzero port");
    let ip = args
        .bind
        .first()
        .copied()
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    ensure!(
        ip.is_loopback(),
        "open requires a loopback address; use serve for remote bindings"
    );
    let addr = SocketAddr::new(ip, args.port);
    if !healthy(addr).await? {
        let logs = dirs::data_local_dir()
            .context("cannot locate data directory")?
            .join("agentbanter/logs");
        fs::create_dir_all(&logs)?;
        let log_path = logs.join("server.log");
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let log = options.open(&log_path)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(server_args(args))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe and does not access Rust state.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
        }
        let mut child = command.spawn().context("start AgentBanter server")?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if healthy(addr).await? {
                break;
            }
            if let Some(status) = child.try_wait()? {
                bail!("server exited with {status}; see {}", log_path.display());
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("server startup timed out; see {}", log_path.display());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let url = format!("http://{addr}");
    println!("AgentBanter: {url}");
    if !args.no_browser {
        browser(&url).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, FromArgMatches};
    #[test]
    fn config_defaults_are_overridden_by_explicit_cli_values() {
        let path =
            std::env::temp_dir().join(format!("agentbanter-settings-{}.json", std::process::id()));
        fs::write(
            &path,
            r#"{"port":9000,"allow_capture":true,"allow_ollama":true,"ollama_url":"http://localhost:11435","ollama_dir":"recordings","bind":["127.0.0.1"],"static_dir":"assets"}"#,
        )
        .unwrap();
        let matches = Args::command().get_matches_from([
            "agentbanter",
            "--config",
            path.to_str().unwrap(),
            "open",
            "--port",
            "9100",
            "--static",
            "explicit",
        ]);
        let mut args = Args::from_arg_matches(&matches).unwrap();
        configure(&mut args, &matches).unwrap();
        assert_eq!(args.port, 9100);
        assert!(args.allow_capture);
        assert!(args.allow_ollama);
        assert_eq!(args.ollama_url, "http://localhost:11435");
        assert_eq!(
            args.ollama_dir,
            Some(path.parent().unwrap().join("recordings"))
        );
        let detached = server_args(&args)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(detached.iter().any(|a| a == "--allow-ollama"));
        assert!(
            detached
                .windows(2)
                .any(|a| a == ["--ollama-url", "http://localhost:11435"])
        );
        assert_eq!(args.static_dir, Some(PathBuf::from("explicit")));
        fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn health_rejects_other_applications() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"app\":\"another-app\"}")
                .await
                .unwrap();
        });
        assert!(
            healthy(addr)
                .await
                .unwrap_err()
                .to_string()
                .contains("different application")
        );
        task.await.unwrap();
    }
}
