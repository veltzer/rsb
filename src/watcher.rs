use anyhow::Result;
use notify::{Event, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::builder::Builder;
use crate::cli::BuildOptions;
use crate::color;

/// Check if a path should be ignored (editor temp files, build artifacts).
/// `output_dir` is the configured global output directory.
fn should_ignore(path: &Path, output_dir: &str) -> bool {
    // Ignore .rsconstruct cache directory (match as a path component, not substring)
    if path.components().any(|c| c.as_os_str() == ".rsconstruct") {
        return true;
    }

    // Ignore the configured output directory (match as a path component, not substring)
    if path.components().any(|c| c.as_os_str() == output_dir) {
        return true;
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        // Editor temp/swap files
        if name.starts_with('.') && name.ends_with(".swp") {
            return true;
        }
        if name.ends_with('~') {
            return true;
        }
        if name.starts_with('#') && name.ends_with('#') {
            return true;
        }
        // Common editor temp patterns
        if name.ends_with(".tmp") {
            return true;
        }
    }

    false
}

/// Register watch paths with the watcher, returning the list of paths being watched.
fn register_watches(
    watcher: &mut impl Watcher,
    paths: &[PathBuf],
) {
    for path in paths {
        let mode = if path.is_dir() {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        // Always warn: an unwatched path silently produces no rebuilds, which
        // is much harder to diagnose than this message.
        if let Err(e) = watcher.watch(path, mode) {
            eprintln!("Warning: could not watch {}: {}", path.display(), e);
        }
    }
}

pub fn watch(ctx: &crate::build_context::BuildContext, opts: &BuildOptions, interrupted: Arc<AtomicBool>) -> Result<()> {
    let mut builder = Builder::new_with_overrides(&opts.iset, &opts.pset)?;
    let mut watch_paths = builder.watch_paths();
    let mut output_dir = builder.output_dir().to_string();

    // Register watches BEFORE the initial build: edits made while the (possibly
    // long) initial build runs must queue up as events, not vanish.
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(tx)?;
    register_watches(&mut watcher, &watch_paths);

    // Initial build
    println!("{}", color::bold("Running initial build..."));
    if let Err(e) = builder.build(ctx, opts, Arc::clone(&interrupted), Vec::new()) {
        println!("{}", color::red(&format!("Initial build error: {e}")));
    }
    drop(builder);

    println!("{}", color::green("Watching for changes... (Ctrl+C to stop)"));

    let debounce_duration = Duration::from_millis(200);
    let poll_interval = Duration::from_millis(500);

    loop {
        // Wait for a relevant file-change event, periodically checking the interrupted flag.
        // Breaks with `true` on a real event, `false` if the watcher channel disconnects.
        let got_event = loop {
            if interrupted.load(Ordering::SeqCst) {
                return Ok(());
            }
            match rx.recv_timeout(poll_interval) {
                Ok(Ok(event)) => {
                    let all_ignored = event.paths.iter().all(|p| should_ignore(p, &output_dir));
                    if all_ignored {
                        continue;
                    }
                    break true;
                }
                Ok(Err(e)) => {
                    println!("{}", color::red(&format!("Watch error: {e}")));
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break false,
            }
        };
        if !got_event {
            break;
        }

        // Debounce: drain further events within the debounce window
        let deadline = Instant::now() + debounce_duration;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Rebuild. A failed builder construction (e.g. rsconstruct.toml saved
        // in a momentarily-invalid state) must not kill watch mode: report it
        // and keep watching — the next save triggers another attempt.
        println!();
        println!("{}", color::bold("Change detected, rebuilding..."));
        match Builder::new_with_overrides(&opts.iset, &opts.pset) {
            Err(e) => {
                println!("{}", color::red(&format!("Config error: {e}")));
            }
            Ok(mut builder) => {
                let new_paths = builder.watch_paths();
                output_dir = builder.output_dir().to_string();
                if let Err(e) = builder.build(ctx, opts, Arc::clone(&interrupted), Vec::new()) {
                    println!("{}", color::red(&format!("Build error: {e}")));
                }

                // Update watches if paths changed (e.g., new scan dirs in config)
                for path in &new_paths {
                    if !watch_paths.contains(path) {
                        register_watches(&mut watcher, std::slice::from_ref(path));
                    }
                }
                // Unwatch paths that are no longer relevant
                for path in &watch_paths {
                    if !new_paths.contains(path) {
                        let _ = watcher.unwatch(path);
                    }
                }
                watch_paths = new_paths;
            }
        }

        println!("{}", color::green("Watching for changes... (Ctrl+C to stop)"));
    }

    Ok(())
}
