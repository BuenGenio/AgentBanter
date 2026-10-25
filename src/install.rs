//! User-scoped Linux integration. No root service or shell command interpolation.
use crate::{InstallAction, ServiceAction};
use anyhow::{Context, Result, ensure};
use std::{fs, path::Path, process::Command};

fn path_text(path: &Path) -> Result<&str> {
    let path = path.to_str().context("launcher paths must be UTF-8")?;
    ensure!(
        !path.chars().any(|c| c.is_control()),
        "launcher paths cannot contain control characters"
    );
    Ok(path)
}
fn desktop_arg(path: &Path) -> Result<String> {
    let path = path_text(path)?;
    ensure!(
        !path.contains('='),
        "desktop executable paths cannot contain '='"
    );
    // Exec quoting is applied after desktop string-value unescaping.
    let quoted = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    Ok(format!(
        "\"{}\"",
        quoted.replace('\\', "\\\\").replace('%', "%%")
    ))
}
fn desktop_entry(exe: &Path) -> Result<String> {
    Ok(include_str!("../packaging/agentbanter.desktop").replace(
        "Exec=agentbanter open",
        &format!("Exec={} open", desktop_arg(exe)?),
    ))
}
fn systemd_arg(path: &Path) -> Result<String> {
    Ok(format!(
        "\"{}\"",
        path_text(path)?
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    ))
}
fn unit(exe: &Path, config: Option<&Path>) -> Result<String> {
    let config = if let Some(config) = config {
        format!(" --config {}", systemd_arg(config)?)
    } else {
        String::new()
    };
    Ok(format!(
        "[Unit]\nDescription=AgentBanter conversation workbench\n\n[Service]\nType=exec\nExecStart={} serve{}\nRestart=on-failure\nRestartSec=3\nUMask=0077\n\n[Install]\nWantedBy=default.target\n",
        systemd_arg(exe)?,
        config
    ))
}
fn write_owned(path: &Path, body: &str) -> Result<()> {
    if path.exists() {
        ensure!(
            fs::read_to_string(path)?.contains("AgentBanter")
                || path.file_name().is_some_and(|n| n == "agentbanter.svg"),
            "refusing to overwrite unrelated file {}",
            path.display()
        );
    }
    fs::create_dir_all(path.parent().context("missing parent directory")?)?;
    fs::write(path, body)?;
    Ok(())
}
fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn desktop(action: InstallAction) -> Result<()> {
    ensure!(
        cfg!(target_os = "linux"),
        "desktop launchers are for Linux; on macOS use agentbanter open"
    );
    let data = dirs::data_dir().context("cannot locate XDG data directory")?;
    let entry = data.join("applications/agentbanter.desktop");
    let icon = data.join("icons/hicolor/scalable/apps/agentbanter.svg");
    match action {
        InstallAction::Install => {
            let exe = std::env::current_exe()?;
            write_owned(&icon, include_str!("../packaging/agentbanter.svg"))?;
            write_owned(&entry, &desktop_entry(&exe)?)?;
            println!(
                "Installed {}\nAgentBanter is available in the GNOME/KDE application menu.",
                entry.display()
            );
        }
        InstallAction::Uninstall => {
            remove(&entry)?;
            remove(&icon)?;
            println!("Removed AgentBanter desktop launcher");
        }
    }
    Ok(())
}
fn systemctl(arguments: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .status()
        .context("run systemctl --user (a systemd user session is required)")?;
    ensure!(
        status.success(),
        "systemctl --user {} exited with {status}",
        arguments.join(" ")
    );
    Ok(())
}
pub fn service(action: ServiceAction, explicit: Option<&Path>) -> Result<()> {
    ensure!(
        cfg!(target_os = "linux"),
        "on macOS manage the user service with brew services start/stop agentbanter"
    );
    let path = dirs::config_dir()
        .context("cannot locate XDG config directory")?
        .join("systemd/user/agentbanter.service");
    match action {
        ServiceAction::Install => {
            let config = explicit
                .map(|p| std::env::current_dir().map(|cwd| cwd.join(p)))
                .transpose()?;
            write_owned(&path, &unit(&std::env::current_exe()?, config.as_deref())?)?;
            systemctl(&["daemon-reload"])?;
            println!(
                "Installed {}\nStart: agentbanter service start\nAt login: systemctl --user enable agentbanter.service",
                path.display()
            );
        }
        ServiceAction::Uninstall => {
            if path.exists() {
                systemctl(&["disable", "--now", "agentbanter.service"])?;
                remove(&path)?;
                systemctl(&["daemon-reload"])?;
            }
        }
        ServiceAction::Start => systemctl(&["start", "agentbanter.service"])?,
        ServiceAction::Stop => systemctl(&["stop", "agentbanter.service"])?,
        ServiceAction::Status => systemctl(&["status", "--no-pager", "agentbanter.service"])?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launchers_quote_paths_without_shell_expansion() {
        let exe = Path::new("/home/A User/50%/$bin/agentbanter");
        let entry = desktop_entry(exe).unwrap();
        assert!(entry.contains("Exec=\"/home/A User/50%%/\\\\$bin/agentbanter\" open"));
        let service = unit(exe, Some(Path::new("/home/A User/config.json"))).unwrap();
        assert!(service.contains("50%%/$$bin"));
        assert!(service.contains("--config \"/home/A User/config.json\""));
        assert!(!service.contains("User="));
    }
    #[test]
    fn rejects_launcher_line_injection() {
        assert!(desktop_entry(Path::new("/bin/agent\nExec=bad")).is_err());
        assert!(unit(Path::new("/bin/agent\nExecStart=bad"), None).is_err());
    }
}
