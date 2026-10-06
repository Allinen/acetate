//! The mirror as a service: `acetate-desktop mirror watch` started with the session, so
//! the mirror keeps up while the app is closed (the app takes over while it's open, see
//! `cli`). Linux: a systemd user unit; Windows: a task run at logon; macOS: a launch
//! agent. Each runs this very program, where it is now: moving or replacing it means
//! installing the service again.

use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    NotInstalled,
    Installed { running: bool },
    /// Not on this system (only made on systems other than Linux, Windows and macOS).
    #[allow(dead_code)]
    Unsupported,
}

/// Runs a system command, failing with what it said.
fn run(command: &mut Command) -> anyhow::Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        command.creation_flags(0x0800_0000);
    }
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!("{program} failed: {said}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn program() -> anyhow::Result<PathBuf> {
    std::env::current_exe().context("finding this program")
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    const UNIT: &str = "acetate-mirror.service";

    fn unit_path() -> PathBuf {
        dirs::config_dir().unwrap_or_default().join("systemd/user").join(UNIT)
    }

    fn systemctl() -> Command {
        let mut command = Command::new("systemctl");
        command.arg("--user");
        command
    }

    /// A path as systemd reads it in `ExecStart` (quoted; `\` and `"` escaped, `%` doubled).
    fn quoted(path: &std::path::Path) -> String {
        let text = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
        format!("\"{text}\"")
    }

    pub fn state() -> State {
        if !unit_path().exists() {
            return State::NotInstalled;
        }
        let running = systemctl().args(["is-active", "--quiet", UNIT]).status().is_ok_and(|s| s.success());
        State::Installed { running }
    }

    pub fn install() -> anyhow::Result<()> {
        let unit = format!(
            "[Unit]\n\
             Description=Acetate mirror: keeps the server's files on this computer\n\
             After=network-online.target\n\n\
             [Service]\n\
             ExecStart={} mirror watch\n\
             Restart=on-failure\n\
             RestartSec=30\n\n\
             [Install]\n\
             WantedBy=default.target\n",
            quoted(&program()?)
        );
        let path = unit_path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, unit).with_context(|| format!("writing {}", path.display()))?;
        run(systemctl().arg("daemon-reload"))?;
        run(systemctl().args(["enable", "--now", UNIT]))?;
        Ok(())
    }

    pub fn remove() -> anyhow::Result<()> {
        let _ = run(systemctl().args(["disable", "--now", UNIT]));
        match std::fs::remove_file(unit_path()) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
            _ => {}
        }
        run(systemctl().arg("daemon-reload"))?;
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    const TASK: &str = "Acetate mirror";

    pub fn state() -> State {
        match run(Command::new("schtasks").args(["/Query", "/TN", TASK, "/FO", "CSV", "/NH"])) {
            Ok(answer) => State::Installed { running: answer.contains("Running") },
            Err(_) => State::NotInstalled,
        }
    }

    pub fn install() -> anyhow::Result<()> {
        let action = format!("\"{}\" mirror watch", program()?.display());
        run(Command::new("schtasks").args(["/Create", "/TN", TASK, "/TR", &action, "/SC", "ONLOGON", "/RL", "LIMITED", "/F"]))?;
        run(Command::new("schtasks").args(["/Run", "/TN", TASK]))?;
        Ok(())
    }

    pub fn remove() -> anyhow::Result<()> {
        let _ = run(Command::new("schtasks").args(["/End", "/TN", TASK]));
        run(Command::new("schtasks").args(["/Delete", "/TN", TASK, "/F"]))?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    const LABEL: &str = "allinen.acetate.mirror";

    fn plist_path() -> PathBuf {
        dirs::home_dir().unwrap_or_default().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
    }

    fn escaped(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    pub fn state() -> State {
        if !plist_path().exists() {
            return State::NotInstalled;
        }
        let running = run(Command::new("launchctl").args(["list", LABEL])).is_ok_and(|answer| answer.contains("\"PID\""));
        State::Installed { running }
    }

    pub fn install() -> anyhow::Result<()> {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array><string>{}</string><string>mirror</string><string>watch</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
</dict>
</plist>
"#,
            escaped(&program()?.to_string_lossy())
        );
        let path = plist_path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, plist)?;
        run(Command::new("launchctl").arg("load").arg("-w").arg(&path))?;
        Ok(())
    }

    pub fn remove() -> anyhow::Result<()> {
        let path = plist_path();
        let _ = run(Command::new("launchctl").arg("unload").arg("-w").arg(&path));
        match std::fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;

    pub fn state() -> State {
        State::Unsupported
    }

    pub fn install() -> anyhow::Result<()> {
        anyhow::bail!("not on this system: run `acetate-desktop mirror watch` at startup yourself")
    }

    pub fn remove() -> anyhow::Result<()> {
        Ok(())
    }
}

/// Whether the service is installed, and running. Asks the system (blocking, briefly).
pub fn state() -> State {
    imp::state()
}

/// Installs the service and starts it.
pub fn install() -> anyhow::Result<()> {
    imp::install()
}

/// Stops the service and removes it.
pub fn remove() -> anyhow::Result<()> {
    imp::remove()
}
