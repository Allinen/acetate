//! `acetate-desktop mirror …`: the mirror without the window, for a service (`watch`)
//! or by hand. Uses the app's root (`ACETATE_ROOT` to pick another) and its login.

use std::time::Duration;

use super::{Doing, Mirror};

const USAGE: &str = "\
acetate-desktop mirror <command>

  status            what's mirrored, what's missing, what could be removed
  sync              bring the mirror up to date, then stop
  watch             keep it up to date (for a service): steps aside while the app
                    is open (the app mirrors then), waits while not logged in or
                    while mirroring is off
  rehash            hash every blob here again
  removable         list blobs nothing wants anymore (the server let them go)
  remove <hash>...  remove those (or `remove all`)
  match <folder>    which of the server's folders a folder here has the files of
  set <key>=<value> enabled=yes|no  volumes=main,archive  previous=yes|no
                    look-in=<folder>=<server folder>[,...]  (empty: none)
                    check-hashes=yes|no  (hash files taken in from look-in folders)

The app's folder is used ($ACETATE_ROOT for another), with the app's login.";

/// Runs a mirror command (the arguments after `mirror`). The process's exit code.
pub fn run(args: &[String]) -> i32 {
    let Some(command) = args.first() else {
        println!("{USAGE}");
        return 2;
    };
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    if command == "watch" && args.len() == 1 {
        return runtime.block_on(watch());
    }
    let root = crate::root::root();
    let api = crate::api::Api::new(root.clone());
    if api.session().is_none() {
        eprintln!("not logged in: log in with the app first");
        return 1;
    }
    let mirror = match Mirror::open(&root, api) {
        Ok(mirror) => std::sync::Arc::new(mirror),
        Err(err) => {
            eprintln!("{err:#}");
            return 1;
        }
    };
    let result = runtime.block_on(async {
        match (command.as_str(), &args[1..]) {
            ("status", []) => {
                mirror.refresh(None).await?;
                print_status(&mirror)
            }
            ("sync", []) => {
                let shown = show_progress(mirror.clone());
                let synced = mirror.sync().await;
                shown.abort();
                print_run(&mirror);
                synced?;
                print_status(&mirror)
            }
            ("rehash", []) => {
                let shown = show_progress(mirror.clone());
                let report = mirror.rehash().await?;
                shown.abort();
                println!("{} blobs hashed: {} set aside, {} not here", report.checked, report.set_aside.len(), report.missing);
                Ok(())
            }
            ("match", [folder]) => {
                mirror.refresh(None).await?;
                let found = mirror.detect(std::path::Path::new(folder), &|| false)?;
                match found.server_folder {
                    Some(server_folder) => println!(
                        "{} of its {} files are the server's, under {server_folder}: look-in={folder}={server_folder}",
                        found.matched, found.files
                    ),
                    None => println!("none of its {} files are the server's (by path and size)", found.files),
                }
                Ok(())
            }
            ("removable", []) => {
                mirror.refresh(None).await?;
                for r in mirror.removable()? {
                    let why = r.state.as_deref().unwrap_or("no mirror wants it");
                    println!("{}  {:>10}  {why}{}", r.sha256, bytes(r.size as u64), r.volume.map(|v| format!(" (on {v})")).unwrap_or_default());
                }
                Ok(())
            }
            ("remove", hashes) if !hashes.is_empty() => {
                let hashes: Vec<String> = if hashes == ["all"] {
                    mirror.removable()?.into_iter().map(|r| r.sha256).collect()
                } else {
                    hashes.to_vec()
                };
                println!("{} removed", mirror.remove(&hashes)?);
                Ok(())
            }
            ("set", settings) if !settings.is_empty() => {
                let mut config = mirror.config()?;
                for setting in settings {
                    let (key, value) = setting.split_once('=').ok_or_else(|| anyhow::anyhow!("{setting}: not key=value"))?;
                    let list = || value.split(',').map(str::trim).filter(|v| !v.is_empty());
                    match key {
                        "enabled" => config.enabled = yes(value)?,
                        "previous" => config.previous = yes(value)?,
                        "volumes" => config.volumes = list().map(String::from).collect(),
                        "look-in" => {
                            config.look_in = list()
                                .map(|pair| {
                                    let (folder, server_folder) =
                                        pair.split_once('=').ok_or_else(|| anyhow::anyhow!("{pair}: <folder>=<server folder>"))?;
                                    Ok(super::index::LookIn { folder: folder.into(), server_folder: server_folder.into() })
                                })
                                .collect::<anyhow::Result<_>>()?
                        }
                        "check-hashes" => config.check_hashes = yes(value)?,
                        _ => anyhow::bail!("{key}: not a setting (enabled, volumes, previous, look-in, check-hashes)"),
                    }
                }
                mirror.set_config(&config)?;
                println!("{config:#?}");
                Ok(())
            }
            _ => {
                println!("{USAGE}");
                Ok(())
            }
        }
    });
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err:#}");
            1
        }
    }
}

/// `watch`: keeps the mirror up to date until Ctrl+C (or the service is stopped). The
/// app being open, not being logged in or mirroring being off: waits (saying so once),
/// and looks again every few seconds. The root, login and settings are read again each
/// time it starts mirroring (the app may have changed them meanwhile).
async fn watch() -> i32 {
    let quit = std::sync::Arc::new(tokio::sync::Notify::new());
    let quitting = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (quit, quitting) = (quit.clone(), quitting.clone());
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            quitting.store(true, std::sync::atomic::Ordering::Relaxed);
            quit.notify_waiters();
        });
    }
    let quit_now = || quitting.load(std::sync::atomic::Ordering::Relaxed);
    // Sleeps, unless asked to quit meanwhile (then false).
    let pause = |seconds: u64| {
        let quit = quit.clone();
        let quitting = quitting.clone();
        async move {
            let notified = quit.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if quitting.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(seconds)) => true,
                _ = notified => false,
            }
        }
    };
    let mut said = String::new();
    let mut say = |text: String| {
        if text != said {
            println!("{text}");
            said = text;
        }
    };
    loop {
        if quit_now() {
            return 0;
        }
        if crate::root::app_is_open() {
            say("The app is open: it mirrors meanwhile.".into());
            if !pause(3).await {
                return 0;
            }
            continue;
        }
        let root = crate::root::root();
        let api = crate::api::Api::new(root.clone());
        if api.session().is_none() {
            say("Not logged in: log in with the app.".into());
            if !pause(30).await {
                return 0;
            }
            continue;
        }
        let mirror = match Mirror::open(&root, api) {
            Ok(mirror) => std::sync::Arc::new(mirror),
            Err(err) => {
                say(format!("{err:#}"));
                if !pause(5).await {
                    return 0;
                }
                continue;
            }
        };
        if !mirror.config().is_ok_and(|c| c.enabled) {
            say("Mirroring is off (Settings, Mirror).".into());
            drop(mirror);
            if !pause(30).await {
                return 0;
            }
            continue;
        }
        say(format!("Mirroring {} into {}.", mirror.server().unwrap_or_default(), root.display()));
        let shown = show_progress(mirror.clone());
        let watching = tokio::spawn({
            let mirror = mirror.clone();
            async move { mirror.watch().await }
        });
        // Until the app opens (it takes over), or asked to quit.
        loop {
            if !pause(3).await || crate::root::app_is_open() || watching.is_finished() {
                mirror.stop();
                break;
            }
        }
        let _ = watching.await;
        shown.abort();
        drop(mirror);
    }
}

fn yes(value: &str) -> anyhow::Result<bool> {
    match value {
        "yes" | "on" | "true" => Ok(true),
        "no" | "off" | "false" => Ok(false),
        _ => anyhow::bail!("{value}: yes or no"),
    }
}

/// What a run did, and its problems.
fn print_run(mirror: &Mirror) {
    let status = mirror.status.lock().unwrap().clone();
    for problem in &status.problems {
        eprintln!("problem: {problem}");
    }
    println!("{} downloaded, {} taken in from the folders looked in", status.downloaded, status.taken_in);
}

fn print_status(mirror: &Mirror) -> anyhow::Result<()> {
    let config = mirror.config()?;
    let plan = mirror.plan()?;
    let volumes: Vec<String> = mirror
        .server_volumes()?
        .into_iter()
        .map(|v| format!("{}{}{}", v.name, if v.default { " (default)" } else { "" }, if config.volumes.contains(&v.name) { ": mirrored" } else { "" }))
        .collect();
    println!("server:     {}", mirror.server()?);
    println!("mirroring:  {}", if config.enabled { "yes" } else { "no" });
    println!("volumes:    {}", volumes.join(", "));
    println!("previous:   {}", if config.previous { "yes" } else { "no" });
    println!(
        "look in:    {}",
        config.look_in.iter().map(|l| format!("{} (the server's {})", l.folder.display(), l.server_folder)).collect::<Vec<_>>().join(", ")
    );
    println!("check hashes of files taken in: {}", if config.check_hashes { "yes" } else { "no" });
    println!("wanted:     {} blobs, {}", plan.wanted.blobs, bytes(plan.wanted.bytes));
    println!("here:       {} blobs, {}", plan.had.blobs, bytes(plan.had.bytes));
    println!("missing:    {} blobs, {}{}", plan.missing.blobs, bytes(plan.missing.bytes), if plan.missing_unsized > 0 { format!(" (+{} of unknown size)", plan.missing_unsized) } else { String::new() });
    println!("removable:  {} blobs, {}", plan.removable.blobs, bytes(plan.removable.bytes));
    println!("unchecked:  {} blobs, {} (taken in by path and size: rehash checks them)", plan.unchecked.blobs, bytes(plan.unchecked.bytes));
    if let Some(free) = plan.free_bytes {
        println!("free space: {}", bytes(free));
    }
    Ok(())
}

/// Prints what the mirror is doing every few seconds, and each new problem.
fn show_progress(mirror: std::sync::Arc<Mirror>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut problems = 0;
        let mut last = String::new();
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let status = mirror.status.lock().unwrap().clone();
            for problem in status.problems.iter().skip(problems.min(status.problems.len())) {
                eprintln!("problem: {problem}");
            }
            problems = status.problems.len();
            let line = match status.doing {
                Doing::Idle | Doing::Waiting => format!("{:?}: {} downloaded, {} taken in", status.doing, status.downloaded, status.taken_in),
                _ => format!("{:?}: {}/{}, {}/{}", status.doing, status.done, status.of, bytes(status.bytes), bytes(status.of_bytes)),
            };
            if line != last {
                println!("{line}");
                last = line;
            }
        }
    })
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}
