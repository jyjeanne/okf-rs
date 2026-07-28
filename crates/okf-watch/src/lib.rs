//! Continuous re-indexing during local development: watches a project's
//! source tree and keeps its OKF bundle up to date as files change,
//! reusing the same incremental-indexing cache `okf-rs generate` uses
//! (see `okf_analyzer::AnalysisCache`) so an edit anywhere in the tree
//! costs a directory scan plus reparsing just the changed files, never a
//! full from-scratch analysis.

use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use okf_analyzer::{AnalysisCache, IncrementalStats};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

/// One regenerate cycle worth reporting to the caller: the initial
/// baseline run, or a later run that actually changed the bundle (a file
/// was reparsed, or the concept id set differs from the last reported
/// run — covering additions/removals even when nothing already-cached
/// needed reparsing).
#[derive(Debug, Clone)]
pub struct RegenerateEvent {
    pub concepts: usize,
    pub stats: IncrementalStats,
}

/// Watches `project_root` for filesystem changes and keeps `bundle_output`
/// up to date, persisting its incremental-indexing cache at `cache_path`
/// across runs the same way `okf-rs generate` does. Regenerates once
/// immediately (the baseline), then again each time a burst of filesystem
/// activity settles for `debounce` — coalescing a flurry of individual
/// events (an editor's atomic-save dance, a `git checkout` touching many
/// files at once) into a single re-scan rather than one per event.
///
/// `on_regenerate` is called for the baseline run and for any later run
/// that actually changed something. A wakeup caused by activity under an
/// ignored path (e.g. `target/`, mid-`cargo build`) still costs a cheap
/// re-scan — the scan itself is `.gitignore`-aware and never descends
/// into it — and a harmless, byte-identical bundle rewrite, but is not
/// reported, so watch mode doesn't spam the caller for unrelated churn
/// elsewhere in the tree.
///
/// Blocks forever — the caller is expected to run this until the process
/// is interrupted (e.g. Ctrl+C) — returning only if the watcher's
/// underlying channel unexpectedly closes.
pub fn watch(
    project_root: &Path,
    bundle_output: &Path,
    cache_path: &Path,
    debounce: Duration,
    mut on_regenerate: impl FnMut(&RegenerateEvent),
) -> Result<()> {
    let (tx, rx) = channel::<notify::Result<Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .context("failed to start filesystem watcher")?;
    watcher
        .watch(project_root, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", project_root.display()))?;

    let mut cache = AnalysisCache::load(cache_path);
    let mut last_ids: Option<BTreeSet<String>> = None;
    regenerate_once(
        project_root,
        bundle_output,
        cache_path,
        &mut cache,
        &mut last_ids,
        &mut on_regenerate,
    )?;

    loop {
        // Block until at least one filesystem event arrives...
        if rx.recv().is_err() {
            return Ok(()); // watcher dropped; nothing more to watch
        }
        // ...then keep draining until a quiet period of `debounce` passes,
        // so a burst of events triggers exactly one regenerate.
        loop {
            match rx.recv_timeout(debounce) {
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
        regenerate_once(
            project_root,
            bundle_output,
            cache_path,
            &mut cache,
            &mut last_ids,
            &mut on_regenerate,
        )?;
    }
}

fn regenerate_once(
    project_root: &Path,
    bundle_output: &Path,
    cache_path: &Path,
    cache: &mut AnalysisCache,
    last_ids: &mut Option<BTreeSet<String>>,
    on_regenerate: &mut impl FnMut(&RegenerateEvent),
) -> Result<()> {
    let project = okf_core::Project::load(project_root)
        .with_context(|| format!("failed to scan {}", project_root.display()))?;
    let (result, stats) = okf_analyzer::analyze_with_cache(&project, cache)?;
    okf_generator::write_bundle(&result.concepts, bundle_output)?;
    cache.save(cache_path)?;

    let ids: BTreeSet<String> = result.concepts.iter().map(|c| c.id.clone()).collect();
    let changed = stats.reparsed > 0 || last_ids.as_ref() != Some(&ids);
    if changed {
        on_regenerate(&RegenerateEvent {
            concepts: result.concepts.len(),
            stats,
        });
    }
    *last_ids = Some(ids);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn setup_project(dir: &Path) {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    }

    /// Spawns `watch` on a background thread against `dir`, collecting
    /// every `RegenerateEvent` into a shared `Vec`. The thread is left
    /// running (there's no shutdown handle) — harmless for a test process,
    /// since `watch` swallows errors from a since-deleted directory and
    /// simply stops making progress once the temp dir is gone.
    fn spawn_watch(dir: &Path, debounce: Duration) -> Arc<Mutex<Vec<RegenerateEvent>>> {
        let events: Arc<Mutex<Vec<RegenerateEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let root = dir.to_path_buf();
        let bundle = dir.join("knowledge");
        let cache_path = dir.join(".okf-cache.json");
        thread::spawn(move || {
            let _ = watch(&root, &bundle, &cache_path, debounce, move |event| {
                events_clone.lock().unwrap().push(event.clone());
            });
        });
        events
    }

    #[test]
    fn reports_baseline_then_silences_a_no_op_rewrite_but_reports_a_real_change() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(dir.path());
        let events = spawn_watch(dir.path(), Duration::from_millis(100));

        thread::sleep(Duration::from_millis(500));
        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "expected exactly the baseline event"
        );

        // Rewriting a file with identical content triggers a wakeup, but
        // nothing to reparse and no id-set change, so it shouldn't report.
        fs::write(dir.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        thread::sleep(Duration::from_millis(700));
        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "identical rewrite shouldn't be reported"
        );

        // A real content change should be reported, with something reparsed.
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn a() {} pub fn b() {}\n",
        )
        .unwrap();
        thread::sleep(Duration::from_millis(700));
        let collected = events.lock().unwrap();
        assert_eq!(collected.len(), 2, "content change should be reported");
        assert!(collected[1].stats.reparsed > 0);
        assert_eq!(collected[1].concepts, 4); // package + module + fn a + fn b
    }

    #[test]
    fn deletion_is_reported_even_though_nothing_needs_reparsing() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(dir.path());
        fs::write(dir.path().join("src/extra.rs"), "pub fn extra() {}\n").unwrap();
        let events = spawn_watch(dir.path(), Duration::from_millis(100));

        thread::sleep(Duration::from_millis(500));
        assert_eq!(events.lock().unwrap().len(), 1);

        fs::remove_file(dir.path().join("src/extra.rs")).unwrap();
        thread::sleep(Duration::from_millis(700));
        let collected = events.lock().unwrap();
        assert_eq!(
            collected.len(),
            2,
            "removing a file should be reported even with nothing to reparse"
        );
        assert_eq!(collected[1].stats.reparsed, 0);
    }
}
