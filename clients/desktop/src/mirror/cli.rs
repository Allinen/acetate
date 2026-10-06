//! `acetate-desktop mirror …`: the mirror without the window, for a service (`watch`)
//! or by hand. Uses the app's root (`ACETATE_ROOT` to pick another) and its login.

use std::time::Duration;

use super::{Doing, Mirror};

const USAGE: &str = "\
acetate-desktop mirror <command>

  status            what's mirrored, what's missing, what could be removed
  sync              bring the mirror up to date, then stop
  watch             bring it up to date and keep it so (for a service)
  rehash            hash every blob here again
  removable         list blobs nothing wants anymore (the server let them go)
  remove <hash>...  remove those (or `remove all`)
  set <key>=<value> enabled=yes|no  volumes=main,archive  previous=yes|no
                    look-in=<folder>[,<folder>...]  (empty: none)

The app's folder is used ($ACETATE_ROOT for another), with the app's login.";

/// Runs a mirror command (the arguments after `mirror`). The process's exit code.
pub fn run(args: &[String]) -> i32 {
    let Some(command) = args.first() else {
        println!("{USAGE}");
        return 2;
    };
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
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
            ("watch", []) => {
                let shown = show_progress(mirror.clone());
                let stopper = mirror.clone();
                tokio::spawn(async move {
                    let _ = tokio::signal::ctrl_c().await;
                    stopper.stop();
                });
                mirror.watch().await;
                shown.abort();
                Ok(())
            }
            ("rehash", []) => {
                let shown = show_progress(mirror.clone());
                let report = mirror.rehash().await?;
                shown.abort();
                println!("{} blobs hashed: {} set aside, {} not here", report.checked, report.set_aside.len(), report.missing);
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
                        "look-in" => config.look_in = list().map(std::path::PathBuf::from).collect(),
                        _ => anyhow::bail!("{key}: not a setting (enabled, volumes, previous, look-in)"),
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
    println!("look in:    {}", config.look_in.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
    println!("wanted:     {} blobs, {}", plan.wanted.blobs, bytes(plan.wanted.bytes));
    println!("here:       {} blobs, {}", plan.had.blobs, bytes(plan.had.bytes));
    println!("missing:    {} blobs, {}{}", plan.missing.blobs, bytes(plan.missing.bytes), if plan.missing_unsized > 0 { format!(" (+{} of unknown size)", plan.missing_unsized) } else { String::new() });
    println!("removable:  {} blobs, {}", plan.removable.blobs, bytes(plan.removable.bytes));
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
