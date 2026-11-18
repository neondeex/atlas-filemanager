//! Linux Search Daemon using Unix Domain Sockets and In-Memory Compact ArenaTree.
// reason: a background daemon with no UI thread; blocking file I/O is what it does.
#![allow(clippy::disallowed_methods)]

use crate::arena_index::{ArenaTree, ROOT_PARENT_ID};
use anyhow::Context as _;
use atlas_core::search::catalog::{SearchBackendStatus, SearchBatch, SearchProgress};
use atlas_core::search::service::{ServiceRequest, ServiceResponse, PROTOCOL_VERSION};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

/// Determines the canonical Unix domain socket path for Atlas search service.
pub fn linux_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("atlas-search.sock")
    } else {
        let user = std::env::var("USER").unwrap_or_else(|_| "default".into());
        PathBuf::from(format!("/tmp/atlas-search-{user}.sock"))
    }
}

/// Directories that must never be indexed (virtual / kernel filesystems).
const EXCLUDED_DIRS: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/tmp",
    "/var/tmp",
    "/var/run",
    "/var/lock",
];

/// Subdirectory names to skip to prevent bloating the index.
const SKIPPED_SUBDIRS: &[&str] = &[
    ".git/objects",
    "node_modules",
    ".cache",
    "target/debug",
    "target/release",
];

pub fn run_daemon(foreground: bool) -> anyhow::Result<()> {
    let sock_path = linux_socket_path();
    tracing::info!(socket = %sock_path.display(), foreground, "starting atlas search daemon");

    // Clean up stale socket if present
    if sock_path.exists() {
        if UnixStream::connect(&sock_path).is_ok() {
            tracing::warn!("atlas search daemon is already running on socket");
            if !foreground {
                return Ok(());
            }
        }
        let _ = fs::remove_file(&sock_path);
    }

    let tree = Arc::new(RwLock::new(ArenaTree::new()));
    let indexing_complete = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));

    // Index in the background, then keep it current (see `keep_index`).
    {
        let tree = Arc::clone(&tree);
        let indexing_complete = Arc::clone(&indexing_complete);
        let running = Arc::clone(&running);
        thread::spawn(move || keep_index(&tree, &indexing_complete, &running));
    }

    // Bind Unix Domain Socket
    let listener = UnixListener::bind(&sock_path)
        .with_context(|| format!("failed to bind unix socket: {}", sock_path.display()))?;

    // Restrict permissions to owner-only (0600) for security (SEC-01)
    let _ = fs::set_permissions(&sock_path, fs::Permissions::from_mode(0o600));

    listener.set_nonblocking(true)?;

    tracing::info!("listening on unix socket: {}", sock_path.display());

    while running.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let tree_clone = Arc::clone(&tree);
                let indexing_complete_clone = Arc::clone(&indexing_complete);

                thread::spawn(move || {
                    if let Err(err) = handle_client(stream, tree_clone, indexing_complete_clone) {
                        tracing::debug!(?err, "client disconnected");
                    }
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                tracing::error!(?e, "error accepting unix socket connection");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    let _ = fs::remove_file(&sock_path);
    Ok(())
}

/// A full re-crawl also catches changes outside the watched home folders and
/// compacts the slots that live updates left deleted.
const FULL_REBUILD: Duration = Duration::from_secs(30 * 60);
/// Changes are applied in batches, so a burst (an unpacked archive) re-reads each
/// changed folder once per batch instead of once per file.
const SETTLE: Duration = Duration::from_millis(500);
/// How deep below each root the crawl goes; live updates keep to the same limit.
const MAX_DEPTH: usize = 12;

/// Builds the index, then keeps it current: folders under the home directory
/// are watched with inotify and re-read when they change, so new, renamed and
/// deleted files show up in search within a second, the way the USN journal
/// keeps the Windows index current. Everything is re-crawled every 30 minutes.
fn keep_index(tree: &Arc<RwLock<ArenaTree>>, complete: &AtomicBool, running: &AtomicBool) {
    use notify::Watcher as _;
    let home = dirs::home_dir();
    // Each changed path, and whether something new now has that name (created or
    // renamed into place), so a folder replaced by another one is re-crawled.
    let (tx, rx) = std::sync::mpsc::channel::<(PathBuf, bool)>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            if !matches!(event.kind, notify::EventKind::Access(_)) {
                let new_name = matches!(
                    event.kind,
                    notify::EventKind::Create(_)
                        | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                );
                for path in event.paths {
                    let _ = tx.send((path, new_name));
                }
            }
        }
    })
    .map_err(|err| {
        tracing::warn!(
            ?err,
            "no file watcher; the index refreshes every 30 minutes"
        )
    })
    .ok();
    // Set when fs.inotify.max_user_watches is used up; each full re-crawl tries again.
    let watch_full = std::cell::Cell::new(false);
    let mut watch = |dir: &Path| {
        let (Some(watcher), Some(home)) = (watcher.as_mut(), home.as_deref()) else {
            return;
        };
        if watch_full.get() || !dir.starts_with(home) {
            return;
        }
        match watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
            Ok(()) => {}
            // fs.inotify.max_user_watches is used up; the periodic rebuild still runs.
            Err(err) if matches!(err.kind, notify::ErrorKind::MaxFilesWatch) => {
                tracing::warn!(dir = %dir.display(), "inotify watch limit reached; no more folders are watched until the next full re-crawl");
                watch_full.set(true);
            }
            // A folder we may not read (root-owned build dirs) or one already gone: skip only it.
            Err(err) => tracing::debug!(?err, dir = %dir.display(), "folder not watched"),
        }
    };

    let mut first = true;
    while running.load(Ordering::Relaxed) {
        let start = Instant::now();
        // The first crawl fills the shared index so searches see partial results;
        // later ones build a fresh index and swap it in.
        let target = if first {
            Arc::clone(tree)
        } else {
            Arc::new(RwLock::new(ArenaTree::new()))
        };
        watch_full.set(false);
        build_index(&target, home.as_deref(), running, &mut watch);
        if !first {
            let fresh = std::mem::take(&mut *target.write().unwrap_or_else(|e| e.into_inner()));
            *tree.write().unwrap_or_else(|e| e.into_inner()) = fresh;
        }
        first = false;
        complete.store(true, Ordering::Relaxed);
        if let Ok(t) = tree.read() {
            tracing::info!(
                indexed_files = t.active_count,
                elapsed_ms = start.elapsed().as_millis(),
                memory_mb = format!("{:.2}", t.memory_usage_bytes() as f64 / 1_048_576.0),
                "crawler finished index"
            );
        }

        let rebuild_at = Instant::now() + FULL_REBUILD;
        while running.load(Ordering::Relaxed) && Instant::now() < rebuild_at {
            let Ok(first_path) = rx.recv_timeout(Duration::from_secs(1)) else {
                continue;
            };
            thread::sleep(SETTLE);
            let mut dirs = std::collections::BTreeSet::new();
            let mut renamed = std::collections::HashSet::new();
            for (path, new_name) in std::iter::once(first_path).chain(rx.try_iter()) {
                dirs.extend(path.parent().map(Path::to_path_buf));
                if new_name {
                    renamed.insert(path);
                }
            }
            for dir in dirs {
                refresh_dir(tree, &dir, &renamed, running, &mut watch);
            }
        }
    }
}

/// Indexes the home folder, then the rest of `/` without the home folder, so
/// no file is listed twice.
fn build_index(
    tree: &Arc<RwLock<ArenaTree>>,
    home: Option<&Path>,
    running: &AtomicBool,
    watch: &mut dyn FnMut(&Path),
) {
    for root in home.into_iter().chain([Path::new("/")]) {
        if !running.load(Ordering::Relaxed) || !root.is_dir() {
            continue;
        }
        let root_id = match tree.write() {
            Ok(mut t) => t.insert_root(root),
            Err(_) => return,
        };
        watch(root);
        let skip = (root == Path::new("/")).then_some(home).flatten();
        index_subtree(tree, root, root_id, skip, MAX_DEPTH, running, watch);
    }
}

/// Adds everything below `dir` (already in the index as `dir_id`), at most
/// `max_depth` levels down.
fn index_subtree(
    tree: &Arc<RwLock<ArenaTree>>,
    dir: &Path,
    dir_id: u32,
    skip: Option<&Path>,
    max_depth: usize,
    running: &AtomicBool,
    watch: &mut dyn FnMut(&Path),
) {
    // walkdir raises a max_depth below min_depth to min_depth, so 0 would still read one level.
    if max_depth == 0 {
        return;
    }
    let walker = walkdir::WalkDir::new(dir)
        .min_depth(1)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| should_index_entry(e.path()) && skip.is_none_or(|s| e.path() != s));
    for entry in walker {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let is_dir = entry.file_type().is_dir();
        let (size, modified) = entry_stats(entry.metadata().ok(), is_dir);
        let Ok(mut t) = tree.write() else { return };
        let parent_id = path
            .parent()
            .and_then(|p| t.dir_paths.get(p).copied())
            .unwrap_or(dir_id);
        let id = t.insert(
            parent_id,
            &entry.file_name().to_string_lossy(),
            is_dir,
            size,
            modified,
        );
        if is_dir {
            t.dir_paths.insert(path.to_path_buf(), id);
            drop(t);
            watch(path);
        }
    }
}

fn entry_stats(metadata: Option<fs::Metadata>, is_dir: bool) -> (u64, Option<u64>) {
    let size = if is_dir {
        0
    } else {
        metadata.as_ref().map_or(0, fs::Metadata::len)
    };
    let modified = metadata
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    (size, modified)
}

/// One entry read from disk by `refresh_dir`: is_dir, size, modified, real name.
type OnDisk = (bool, u64, Option<u64>, std::ffi::OsString);

/// Re-reads one folder after a change and brings its part of the index in line
/// with the disk. A folder that is gone or not indexed yet is handled through
/// its nearest indexed ancestor, which removes or adds it as a whole. A child
/// folder listed in `renamed` (created or renamed into place since the last
/// pass) is re-crawled even when one of the same name was indexed before.
fn refresh_dir(
    tree: &Arc<RwLock<ArenaTree>>,
    dir: &Path,
    renamed: &std::collections::HashSet<PathBuf>,
    running: &AtomicBool,
    watch: &mut dyn FnMut(&Path),
) {
    let mut current = dir.to_path_buf();
    let (dir, dir_id) = loop {
        let known = tree
            .read()
            .ok()
            .and_then(|t| t.dir_paths.get(&current).copied());
        match known {
            Some(id) if current.is_dir() => break (current, id),
            _ if !current.pop() => return,
            _ => {}
        }
    };
    // Levels below the root, so live updates stop where the full crawl stops.
    let depth = {
        let Ok(t) = tree.read() else { return };
        std::iter::successors(Some(dir_id), |&id| {
            Some(t.nodes[id as usize].parent_id).filter(|&parent| parent != ROOT_PARENT_ID)
        })
        .count()
            - 1
    };
    let Ok(read) = fs::read_dir(&dir) else { return };
    // Keyed by the index's (lossy) name; the real name builds paths on disk.
    let mut on_disk: std::collections::HashMap<String, OnDisk> = read
        .flatten()
        .filter(|e| should_index_entry(&e.path()))
        .filter_map(|e| {
            let is_dir = e.file_type().ok()?.is_dir();
            let (size, modified) = entry_stats(e.metadata().ok(), is_dir);
            let real = e.file_name();
            Some((
                real.to_string_lossy().into_owned(),
                (is_dir, size, modified, real),
            ))
        })
        .collect();
    {
        let Ok(mut t) = tree.write() else { return };
        for (id, name, is_dir) in t.children(dir_id) {
            let same = on_disk.get(&name).is_some_and(|(disk_is_dir, _, _, real)| {
                *disk_is_dir == is_dir && !(is_dir && renamed.contains(&dir.join(real)))
            });
            match on_disk.remove(&name) {
                Some((_, size, modified, _)) if same => {
                    let node = &mut t.nodes[id as usize];
                    node.size = size;
                    node.modified = modified;
                }
                Some(entry) => {
                    t.remove_subtree(id);
                    on_disk.insert(name, entry);
                }
                None => t.remove_subtree(id),
            }
        }
    }
    if depth >= MAX_DEPTH {
        return;
    }
    for (name, (is_dir, size, modified, real)) in on_disk {
        let Ok(mut t) = tree.write() else { return };
        let id = t.insert(dir_id, &name, is_dir, size, modified);
        if is_dir {
            let path = dir.join(&real);
            t.dir_paths.insert(path.clone(), id);
            drop(t);
            watch(&path);
            index_subtree(tree, &path, id, None, MAX_DEPTH - depth - 1, running, watch);
        }
    }
}

fn should_index_entry(path: &Path) -> bool {
    let text = path.to_string_lossy();
    !EXCLUDED_DIRS
        .iter()
        .any(|excluded| path.starts_with(excluded))
        && !SKIPPED_SUBDIRS.iter().any(|skipped| text.contains(skipped))
}

fn handle_client(
    mut stream: UnixStream,
    tree: Arc<RwLock<ArenaTree>>,
    indexing_complete: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();

    while reader.read_line(&mut line)? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let request: ServiceRequest = serde_json::from_str(trimmed)
            .with_context(|| format!("invalid request json: {trimmed}"))?;

        let response = match request {
            ServiceRequest::Hello { protocol: _ } => ServiceResponse::Hello {
                protocol: PROTOCOL_VERSION,
                schema: 1,
            },
            ServiceRequest::Status => {
                let (active, is_ready) = match tree.read() {
                    Ok(t) => (
                        t.active_count as u64,
                        indexing_complete.load(Ordering::Relaxed),
                    ),
                    Err(_) => (0, false),
                };
                let status = if is_ready {
                    SearchBackendStatus::Ready
                } else {
                    SearchBackendStatus::Loading
                };
                ServiceResponse::Status(vec![SearchProgress {
                    source: "atlas-linux-daemon".into(),
                    status,
                    indexed: active,
                    total: None,
                }])
            }
            ServiceRequest::Search(query) => {
                let results = match tree.read() {
                    Ok(t) => t.search(&query),
                    Err(_) => Vec::new(),
                };
                ServiceResponse::Search(SearchBatch {
                    generation: query.generation,
                    results,
                    complete: true,
                })
            }
        };

        let mut payload = serde_json::to_vec(&response)?;
        payload.push(b'\n');
        stream.write_all(&payload)?;
        stream.flush()?;

        line.clear();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_core::search::catalog::SearchQuery;

    #[test]
    fn test_daemon_unix_socket_roundtrip() {
        let temp_dir = std::env::temp_dir();
        let sock_path = temp_dir.join(format!("atlas_test_{}.sock", std::process::id()));
        if sock_path.exists() {
            let _ = fs::remove_file(&sock_path);
        }

        let listener = UnixListener::bind(&sock_path).expect("failed to bind test socket");
        let tree = Arc::new(RwLock::new(ArenaTree::new()));
        {
            let mut t = tree.write().expect("lock failed");
            let root = t.insert_root(Path::new("/home/testuser"));
            t.insert(root, "cargo.toml", false, 120, None);
            t.insert(root, "readme.md", false, 300, None);
        }
        let indexing_complete = Arc::new(AtomicBool::new(true));

        let srv_tree = Arc::clone(&tree);
        let srv_complete = Arc::clone(&indexing_complete);
        let srv_sock = sock_path.clone();

        let srv_handle = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let _ = handle_client(stream, srv_tree, srv_complete);
            }
            let _ = fs::remove_file(&srv_sock);
        });

        // Client connects and queries
        let mut client = UnixStream::connect(&sock_path).expect("connect failed");
        let req = ServiceRequest::Search(SearchQuery::text("cargo"));
        let mut payload = serde_json::to_vec(&req).expect("serialize failed");
        payload.push(b'\n');
        client.write_all(&payload).expect("write failed");
        client.flush().expect("flush failed");

        let mut reader = BufReader::new(client);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read failed");
        let resp: ServiceResponse = serde_json::from_str(line.trim()).expect("deserialize failed");

        match resp {
            ServiceResponse::Search(batch) => {
                assert_eq!(batch.results.len(), 1);
                assert_eq!(
                    batch.results[0].path,
                    PathBuf::from("/home/testuser/cargo.toml")
                );
            }
            other => panic!("unexpected response: {:?}", other),
        }

        let _ = srv_handle.join();
    }

    fn names(tree: &Arc<RwLock<ArenaTree>>, text: &str) -> Vec<PathBuf> {
        let mut found: Vec<_> = tree
            .read()
            .unwrap()
            .search(&SearchQuery::text(text))
            .into_iter()
            .map(|r| r.path)
            .collect();
        found.sort();
        found
    }

    #[test]
    fn live_updates_stop_at_the_crawl_depth() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".depth-index-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let deepest = (1..=MAX_DEPTH).fold(base.clone(), |p, level| p.join(format!("d{level}")));
        fs::create_dir_all(&deepest).unwrap();
        fs::write(deepest.join("zeta-too-deep.txt"), "x").unwrap();

        let tree = Arc::new(RwLock::new(ArenaTree::new()));
        let running = AtomicBool::new(true);
        let mut watch = |_: &Path| {};
        let root = tree.write().unwrap().insert_root(&base);
        index_subtree(&tree, &base, root, None, MAX_DEPTH, &running, &mut watch);
        assert!(tree.read().unwrap().dir_paths.contains_key(&deepest));
        assert!(names(&tree, "zeta").is_empty());

        // A change in the deepest folder, and a new folder one level above it,
        // add nothing the full crawl would leave out.
        fs::write(deepest.join("zeta-new-too-deep.txt"), "x").unwrap();
        let above = deepest.parent().unwrap();
        fs::create_dir_all(above.join("fresh/inner")).unwrap();
        fs::write(above.join("zeta-at-limit.txt"), "x").unwrap();
        fs::write(above.join("fresh/zeta-too-deep.txt"), "x").unwrap();
        fs::write(above.join("fresh/inner/zeta-too-deep.txt"), "x").unwrap();
        let none = std::collections::HashSet::new();
        refresh_dir(&tree, &deepest, &none, &running, &mut watch);
        refresh_dir(&tree, above, &none, &running, &mut watch);

        assert_eq!(names(&tree, "zeta"), vec![above.join("zeta-at-limit.txt")]);
        assert!(tree
            .read()
            .unwrap()
            .dir_paths
            .contains_key(&above.join("fresh")));

        // A full crawl of the same folders finds the same files.
        let full = Arc::new(RwLock::new(ArenaTree::new()));
        let root = full.write().unwrap().insert_root(&base);
        index_subtree(&full, &base, root, None, MAX_DEPTH, &running, &mut watch);
        assert_eq!(names(&full, "zeta"), names(&tree, "zeta"));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn changes_on_disk_reach_the_index_through_refresh_dir() {
        // Not under /tmp: the crawler skips it.
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".live-index-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("old/inner")).unwrap();
        fs::write(base.join("old/inner/zeta-note.txt"), "x").unwrap();
        fs::write(base.join("zeta-keep.txt"), "x").unwrap();
        fs::write(base.join("zeta-renamed-from.txt"), "x").unwrap();

        let tree = Arc::new(RwLock::new(ArenaTree::new()));
        let running = AtomicBool::new(true);
        let watched = std::cell::RefCell::new(Vec::new());
        let mut watch = |p: &Path| watched.borrow_mut().push(p.to_path_buf());
        let root = tree.write().unwrap().insert_root(&base);
        index_subtree(&tree, &base, root, None, MAX_DEPTH, &running, &mut watch);
        assert_eq!(names(&tree, "zeta").len(), 3);

        fs::remove_dir_all(base.join("old")).unwrap();
        fs::rename(
            base.join("zeta-renamed-from.txt"),
            base.join("zeta-renamed-to.txt"),
        )
        .unwrap();
        fs::create_dir_all(base.join("new/deeper")).unwrap();
        fs::write(base.join("new/deeper/zeta-fresh.txt"), "x").unwrap();
        // What inotify reports: the parent of each changed path. "old/inner" no longer
        // exists, so it is handled through its nearest indexed ancestor.
        let none = std::collections::HashSet::new();
        for dir in [base.join("old/inner"), base.clone()] {
            refresh_dir(&tree, &dir, &none, &running, &mut watch);
        }

        assert_eq!(
            names(&tree, "zeta"),
            vec![
                base.join("new/deeper/zeta-fresh.txt"),
                base.join("zeta-keep.txt"),
                base.join("zeta-renamed-to.txt"),
            ]
        );
        assert!(watched.borrow().contains(&base.join("new/deeper")));
        assert!(!tree
            .read()
            .unwrap()
            .dir_paths
            .contains_key(&base.join("old")));

        // A folder renamed away and replaced by another one of the same name
        // within one batch: the old contents leave, the new ones are crawled.
        fs::rename(base.join("new"), base.join("moved-away")).unwrap();
        fs::create_dir_all(base.join("new")).unwrap();
        fs::write(base.join("new/zeta-replacement.txt"), "x").unwrap();
        let renamed = std::collections::HashSet::from([base.join("new"), base.join("moved-away")]);
        watched.borrow_mut().clear();
        refresh_dir(&tree, &base, &renamed, &running, &mut watch);
        let found = names(&tree, "zeta");
        assert!(found.contains(&base.join("new/zeta-replacement.txt")));
        assert!(found.contains(&base.join("moved-away/deeper/zeta-fresh.txt")));
        assert!(!found.contains(&base.join("new/deeper/zeta-fresh.txt")));
        assert!(watched.borrow().contains(&base.join("new")));

        // The same with a folder name that is not valid UTF-8.
        let odd =
            base.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"odd-\xff"));
        fs::create_dir_all(odd.join("inner")).unwrap();
        fs::write(odd.join("inner/zeta-odd-old.txt"), "x").unwrap();
        refresh_dir(&tree, &base, &none, &running, &mut watch);
        assert_eq!(names(&tree, "zeta-odd").len(), 1);
        fs::rename(&odd, base.join("odd-away")).unwrap();
        fs::create_dir(&odd).unwrap();
        fs::write(odd.join("zeta-odd-new.txt"), "x").unwrap();
        let renamed = std::collections::HashSet::from([odd.clone(), base.join("odd-away")]);
        refresh_dir(&tree, &base, &renamed, &running, &mut watch);
        let found: Vec<String> = names(&tree, "zeta-odd")
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found
            .iter()
            .any(|p| p.ends_with("odd-\u{fffd}/zeta-odd-new.txt")));
        assert!(found
            .iter()
            .any(|p| p.ends_with("odd-away/inner/zeta-odd-old.txt")));
        assert!(watched.borrow().contains(&odd));
        fs::remove_dir_all(&base).unwrap();
    }
}
