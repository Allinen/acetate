//! Links to things in Acetate, to send to someone who has an account:
//! `https://<server>/open/<track|album|artist>/<id>`. The server's page for it hands it to
//! this app (if the browser is logged in) as `acetate://open/<kind>/<id>?server=<server>`,
//! the address the app is registered for (by `install.sh` on Linux, by itself on Windows;
//! Settings says whether it is).
//!
//! The app runs once: a second start with a link (the system starting it for an
//! `acetate://` address) hands the link to the one running, through a socket in the
//! user's runtime folder (on Windows a named pipe), and ends; the running one comes to the
//! front and opens it.
//!
//! On Windows the app registers itself for the address when it starts, if nothing else
//! that's there has it (`register_if_unclaimed`; Settings can take it too): the user's
//! part of the registry, no admin needed.

use std::sync::mpsc;


pub use acetate_core::links::{Link, Target, parse, same_server, web};

#[cfg(unix)]
fn socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let user = std::env::var("USER").unwrap_or_default();
    dir.join(format!("acetate-desktop-{user}.sock"))
}

#[cfg(windows)]
fn socket_path() -> std::path::PathBuf {
    let user = std::env::var("USERNAME").unwrap_or_default();
    format!(r"\\.\pipe\acetate-desktop-{user}").into()
}

/// Hands `link` to the app if it's running already. True if it took it (then this start
/// has nothing more to do).
pub fn hand_over(link: &str) -> bool {
    #[cfg(any(unix, windows))]
    {
        hand_over_to(&socket_path(), link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = link;
        false
    }
}

#[cfg(unix)]
fn hand_over_to(path: &std::path::Path, link: &str) -> bool {
    use std::io::Write;
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(mut stream) => writeln!(stream, "{link}").is_ok(),
        Err(_) => false,
    }
}

/// Listens for links handed over by later starts: each arrives on the channel, and the
/// window is woken and brought to the front. None if another app is listening already
/// (or it can't listen: then later starts open their own window).
pub fn listen(ctx: eframe::egui::Context) -> Option<mpsc::Receiver<String>> {
    #[cfg(any(unix, windows))]
    {
        listen_at(&socket_path(), ctx)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = ctx;
        None
    }
}

#[cfg(unix)]
fn listen_at(path: &std::path::Path, ctx: eframe::egui::Context) -> Option<mpsc::Receiver<String>> {
    use std::io::BufRead;
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = path.to_path_buf();
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        // There already: another app is listening, or one ended without cleaning up.
        Err(_) if UnixStream::connect(&path).is_ok() => return None,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            UnixListener::bind(&path).ok()?
        }
    };
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if std::io::BufReader::new(stream).read_line(&mut line).is_ok() && !line.trim().is_empty() {
                if send.send(line.trim().to_string()).is_err() {
                    break;
                }
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
                ctx.request_repaint();
            }
        }
    });
    Some(receive)
}

#[cfg(windows)]
fn hand_over_to(path: &std::path::Path, link: &str) -> bool {
    use std::io::Write;
    // Windows lets only the app the user just acted in (the browser, then this start)
    // bring a window to the front: this start passes that on to the running one.
    // SAFETY: no pointers; it only grants a permission.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY) };
    // Busy for a moment (the app is between two callers): try again shortly.
    for _ in 0..20 {
        match std::fs::OpenOptions::new().write(true).open(path) {
            Ok(mut pipe) => return writeln!(pipe, "{link}").is_ok(),
            Err(err) if err.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
    false
}

/// A new instance of the pipe to wait for the next caller on (`first`: fails if the pipe
/// is there already, that is another app is listening). None if it can't be made.
#[cfg(windows)]
fn pipe_instance(name: &[u16], first: bool) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};
    let open = PIPE_ACCESS_INBOUND | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    // SAFETY: `name` ends in a 0; no security attributes (the default: only this user, and
    // admins, may write to it).
    let handle = unsafe { CreateNamedPipeW(name.as_ptr(), open, mode, PIPE_UNLIMITED_INSTANCES, 0, 4096, 0, std::ptr::null()) };
    // SAFETY: a handle just made, owned by the file from here on.
    (handle != INVALID_HANDLE_VALUE).then(|| unsafe { std::fs::File::from_raw_handle(handle) })
}

#[cfg(windows)]
fn listen_at(path: &std::path::Path, ctx: eframe::egui::Context) -> Option<mpsc::Receiver<String>> {
    use std::io::BufRead;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError};
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut waiting = pipe_instance(&name, true)?;
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            // SAFETY: a pipe instance's handle, waited on without overlapping.
            let connected = unsafe { ConnectNamedPipe(waiting.as_raw_handle(), std::ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            // The next caller's instance is there before this one's read, so the pipe is
            // never missing while the app runs (a start then would think it's alone).
            let Some(next) = pipe_instance(&name, false) else { break };
            let caller = std::mem::replace(&mut waiting, next);
            if !connected {
                continue;
            }
            let mut line = String::new();
            if std::io::BufReader::new(caller).read_line(&mut line).is_ok() && !line.trim().is_empty() {
                if send.send(line.trim().to_string()).is_err() {
                    break;
                }
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
                ctx.request_repaint();
            }
        }
    });
    Some(receive)
}

/// What opens `acetate://` links on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handler {
    /// This app.
    Here,
    /// Something else (what the system says it is).
    Other(String),
    Nobody,
}

/// Asks the system what opens `acetate://` links (it may take a moment: run it in the
/// background).
pub fn handler() -> Handler {
    #[cfg(windows)]
    {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Classes\acetate\shell\open\command");
        match key.and_then(|key| key.get_value::<String, _>("")) {
            Ok(command) if Some(&command) == windows_command().as_ref() => Handler::Here,
            Ok(command) => Handler::Other(command),
            Err(_) => Handler::Nobody,
        }
    }
    #[cfg(not(windows))]
    {
        // Linux: what `install.sh` set up (macOS: nothing yet).
        let answer = std::process::Command::new("xdg-mime").args(["query", "default", "x-scheme-handler/acetate"]).output();
        match answer.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default() {
            handler if handler == "acetate.desktop" => Handler::Here,
            handler if handler.is_empty() => Handler::Nobody,
            handler => Handler::Other(handler),
        }
    }
}

/// Whether the app can make itself what opens the links (on Windows; on Linux
/// `install.sh` does it).
pub const CAN_REGISTER: bool = cfg!(windows);

/// The command Windows runs for a link: this program, as it's installed now.
#[cfg(windows)]
fn windows_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" \"%1\"", exe.display()))
}

/// At start (Windows, where nothing installs the app): makes it what opens the links if
/// nothing does, or what does is a program that's gone (the app's folder moved or
/// deleted). Another Acetate that's there keeps them; Settings says so, and can take them.
pub fn register_if_unclaimed() {
    if !CAN_REGISTER {
        return;
    }
    let unclaimed = match handler() {
        Handler::Here => false,
        Handler::Nobody => true,
        Handler::Other(command) => !program(&command).is_some_and(|p| p.exists()),
    };
    if unclaimed && let Err(err) = register() {
        eprintln!("{err}");
    }
}

/// The program a command runs: `"C:\…\Acetate.exe" "%1"` → `C:\…\Acetate.exe`.
fn program(command: &str) -> Option<std::path::PathBuf> {
    let command = command.trim_start();
    let path = match command.strip_prefix('"') {
        Some(rest) => &rest[..rest.find('"')?],
        None => command.split_whitespace().next()?,
    };
    Some(path.into())
}

/// Makes this app what opens `acetate://` links (Windows: in the user's part of the
/// registry, `HKEY_CURRENT_USER\Software\Classes\acetate`).
pub fn register() -> Result<(), String> {
    #[cfg(windows)]
    {
        let command = windows_command().ok_or("couldn't tell where the app is")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let classes = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let set = || -> std::io::Result<()> {
            let (key, _) = classes.create_subkey(r"Software\Classes\acetate")?;
            key.set_value("", &"URL:Acetate")?;
            key.set_value("URL Protocol", &"")?;
            key.create_subkey("DefaultIcon")?.0.set_value("", &format!("\"{}\",0", exe.display()))?;
            key.create_subkey(r"shell\open\command")?.0.set_value("", &command)?;
            Ok(())
        };
        set().map_err(|e| format!("couldn't register for links: {e}"))
    }
    #[cfg(not(windows))]
    {
        Err("install.sh sets up links".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_a_command_runs() {
        assert_eq!(program(r#""C:\Apps\Acetate\Acetate.exe" "%1""#), Some(r"C:\Apps\Acetate\Acetate.exe".into()));
        assert_eq!(program(r#"C:\Acetate.exe "%1""#), Some(r"C:\Acetate.exe".into()));
        assert_eq!(program(""), None);
    }

    /// A second start hands its link to the first, which gets it; a third can't listen
    /// too; a socket left behind by an app that ended is taken over.
    #[cfg(any(unix, windows))]
    #[test]
    fn a_link_is_handed_to_the_app_running() {
        let dir = std::env::temp_dir().join(format!("acetate-links-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        let path = dir.join("app.sock");
        #[cfg(windows)]
        let path = std::path::PathBuf::from(format!(r"\\.\pipe\acetate-links-test-{}", std::process::id()));
        assert!(!hand_over_to(&path, "acetate://open/track/x"), "nothing running yet");

        let ctx = eframe::egui::Context::default();
        let links = listen_at(&path, ctx.clone()).expect("the first listens");
        assert!(listen_at(&path, ctx.clone()).is_none(), "a second doesn't");
        assert!(hand_over_to(&path, "acetate://open/album/y"));
        let got = links.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(got, "acetate://open/album/y");
        // Again: the next caller is waited for too.
        assert!(hand_over_to(&path, "acetate://open/track/z"));
        assert_eq!(links.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), "acetate://open/track/z");

        // Left behind (the listener gone, the file still there): taken over.
        #[cfg(unix)]
        {
        let stale = dir.join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(listen_at(&stale, ctx).is_some());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
