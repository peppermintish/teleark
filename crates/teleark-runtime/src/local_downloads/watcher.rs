//! Native directory notifications. Registration and mount probing never block the controller.
use super::{ResultEvent, Shared, WatchRequest};
use notify::{RecursiveMode, Watcher as _};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::Ordering,
        mpsc::{Receiver, SyncSender},
    },
};

const MAX_EVENT_PATHS: usize = 64;

fn nearest_existing_directory(path: &Path) -> Option<(PathBuf, PathBuf)> {
    path.ancestors().find_map(|ancestor| {
        std::fs::metadata(ancestor)
            .is_ok_and(|meta| meta.is_dir())
            .then(|| {
                std::fs::canonicalize(ancestor)
                    .ok()
                    .map(|resolved| (ancestor.to_path_buf(), resolved))
            })
            .flatten()
    })
}

type PathAliases = Arc<Mutex<BTreeMap<PathBuf, BTreeSet<PathBuf>>>>;

fn logical_event_paths(paths: Vec<PathBuf>, aliases: &PathAliases) -> Vec<PathBuf> {
    let aliases = aliases
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut logical = BTreeSet::new();
    for path in paths {
        logical.insert(path.clone());
        for ancestor in path.ancestors() {
            if let Some(roots) = aliases.get(ancestor)
                && let Ok(suffix) = path.strip_prefix(ancestor)
            {
                for root in roots {
                    logical.insert(root.join(suffix));
                    if logical.len() > MAX_EVENT_PATHS {
                        return logical.into_iter().collect();
                    }
                }
            }
        }
    }
    logical.into_iter().collect()
}

fn new_watcher(
    shared: &Arc<Shared>,
    results: &SyncSender<ResultEvent>,
    aliases: &PathAliases,
) -> notify::Result<notify::RecommendedWatcher> {
    let shared = shared.clone();
    let results = results.clone();
    let aliases = aliases.clone();
    notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
        Ok(event) if !event.paths.is_empty() && event.paths.len() <= MAX_EVENT_PATHS => {
            let paths = logical_event_paths(event.paths, &aliases);
            if paths.len() > MAX_EVENT_PATHS
                || results.try_send(ResultEvent::WatchPaths(paths)).is_err()
            {
                shared.watch_overflow.store(true, Ordering::Release);
                let _ = results.try_send(ResultEvent::Wake);
            }
        }
        Ok(_) => {
            shared.watch_overflow.store(true, Ordering::Release);
            let _ = results.try_send(ResultEvent::Wake);
        }
        Err(_) => {
            shared.watch_failed.store(true, Ordering::Release);
            let _ = results.try_send(ResultEvent::WatchFailure);
        }
    })
}

pub(super) fn run(shared: Arc<Shared>, requests: Receiver<()>, results: SyncSender<ResultEvent>) {
    let mut watcher = None;
    let mut watched = BTreeSet::new();
    let mut active_epoch = None;
    let aliases: PathAliases = Arc::new(Mutex::new(BTreeMap::new()));
    while requests.recv().is_ok() {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        while requests.try_recv().is_ok() {}
        let WatchRequest { epoch, parents } = shared
            .watch_request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if active_epoch != Some(epoch) || shared.watch_restart.swap(false, Ordering::AcqRel) {
            watcher = None;
            watched.clear();
            aliases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            active_epoch = Some(epoch);
        }
        if watcher.is_none() {
            match new_watcher(&shared, &results, &aliases) {
                Ok(ready) => watcher = Some(ready),
                Err(_) => {
                    shared.watch_failed.store(true, Ordering::Release);
                    let _ = results.send(ResultEvent::WatchFailure);
                    continue;
                }
            }
        }
        let Some(watcher) = watcher.as_mut() else {
            continue;
        };
        let requested_roots: BTreeMap<_, _> = parents
            .iter()
            .filter_map(|parent| {
                nearest_existing_directory(parent).map(|root| (parent.clone(), root))
            })
            .collect();
        let wanted: BTreeSet<_> = requested_roots
            .values()
            .map(|(_, resolved)| resolved.clone())
            .collect();
        let new_roots: Vec<_> = wanted.difference(&watched).cloned().collect();
        for root in new_roots {
            if watcher.watch(&root, RecursiveMode::NonRecursive).is_ok() {
                watched.insert(root);
            }
        }
        let stale: Vec<_> = watched.difference(&wanted).cloned().collect();
        for root in stale {
            if watcher.unwatch(&root).is_err() {
                shared.watch_failed.store(true, Ordering::Release);
                let _ = results.try_send(ResultEvent::WatchFailure);
            }
            watched.remove(&root);
        }
        let roots: BTreeMap<_, _> = requested_roots
            .into_iter()
            .filter_map(|(parent, (logical, resolved))| {
                watched.contains(&resolved).then_some((parent, logical))
            })
            .collect();
        let mut next_aliases: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
        for (parent, logical) in &roots {
            if let Ok(resolved) = std::fs::canonicalize(logical) {
                next_aliases
                    .entry(resolved)
                    .or_default()
                    .insert(logical.clone());
            } else if parent == logical {
                // The directory may have vanished immediately after registration.
                shared.watch_overflow.store(true, Ordering::Release);
            }
        }
        *aliases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next_aliases;
        if results
            .send(ResultEvent::WatchStatus { epoch, roots })
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_events_map_back_to_the_recorded_directory_alias() {
        let aliases = Arc::new(Mutex::new(BTreeMap::from([(
            PathBuf::from("/private/tmp/downloads"),
            BTreeSet::from([PathBuf::from("/tmp/downloads")]),
        )])));
        let paths = logical_event_paths(
            vec![PathBuf::from("/private/tmp/downloads/file.bin")],
            &aliases,
        );
        assert!(paths.contains(&PathBuf::from("/tmp/downloads/file.bin")));
    }
}
