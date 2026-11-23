//! File change notification (ADR-782): a process watches a file or a directory and gets a
//! `[:file-watch handle path kind]` message when it changes, instead of polling `file/size`.
//!
//! One OS watcher serves the whole OS process — `notify`'s recommended backend: inotify on
//! Linux, FSEvents on macOS, `ReadDirectoryChangesW` on Windows — with one thread of its own.
//! Every handle is routed through it, so a program watching a thousand files holds one
//! inotify instance (Linux allows 128 per user by default), not a thousand.
//!
//! **A file is watched through its directory.** `(file/watch "/a/b.log")` puts the OS watch
//! on `/a` and keeps the events whose name is `b.log`. That is what makes a path that does
//! not exist yet watchable (its creation is the first message), and what keeps a watch alive
//! across a delete-and-recreate or an editor's save-by-rename, which replace the file's inode
//! (an inode watch would follow the old file and go silent). A directory is watched itself,
//! non-recursively, and its messages name the entry that changed. The directory must exist.
//!
//! **Bursts coalesce.** A handle has at most one message per (path, kind) waiting in its
//! owner's mailbox: an event that finds one still queued is dropped, and the next event after
//! the owner receives it sends a fresh one. So a message means "this path changed since you
//! last heard about it", and a follower re-reads from its offset on each — never once per
//! write. The mechanism is the mailbox's delivery credit (the one socket backpressure uses),
//! which returns when the message leaves the mailbox by any path.
//!
//! **Unwatch is final.** `file/unwatch` drops the OS watch when no other handle needs that
//! directory, and marks the handle closed: a message already queued for it is withdrawn, so
//! a `receive` after `file/unwatch` returns never sees one. A handle belongs to the process
//! that made it and ends when that process exits (`close_process_watches`, beside sockets and
//! child processes in the scheduler's retirement).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use crate::core::value;
use crate::host::owned::OwnedTable;
use crate::process::{CreditSource, DeliveryCredit, Message, deliver_credited};

/// What happened to a path. Coarse on purpose: the backends disagree on the finer kinds, and
/// a watcher re-reads the path's state anyway.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Change {
    Created,
    Modified,
    Removed,
}

impl Change {
    fn keyword(self) -> &'static str {
        match self {
            Change::Created => "created",
            Change::Modified => "modified",
            Change::Removed => "removed",
        }
    }
}

/// One `file/watch` handle.
struct Watch {
    /// The handle's ref id.
    id: u64,
    /// The process the messages go to.
    owner: u64,
    /// The directory the OS watch is on, canonical — the form the backend reports events in.
    directory: PathBuf,
    /// `Some(name)` for a file watch (only that entry's events), `None` for a directory watch.
    name: Option<std::ffi::OsString>,
    /// The path as the caller spelled it, which every message reports (a directory watch
    /// joins the changed entry's name onto it).
    spelled: PathBuf,
    /// Set by unwatch or the owner's exit: no further message is sent, and one still queued
    /// is withdrawn.
    closed: AtomicBool,
    /// The (path, change) pairs with a message waiting in the owner's mailbox.
    queued: Mutex<HashSet<(String, Change)>>,
}

/// A queued message's claim on its (path, change) slot: released when the message leaves
/// the mailbox, withdrawn once the handle is closed.
struct Queued {
    watch: Arc<Watch>,
    key: (String, Change),
}

impl CreditSource for Queued {
    fn release(&self, _cost: usize) {
        crate::core::sync::lock(&self.watch.queued).remove(&self.key);
    }

    fn withdrawn(&self) -> bool {
        self.watch.closed.load(Ordering::Acquire)
    }
}

/// The OS watcher and the bookkeeping that decides when a directory needs one. Every watch
/// and unwatch runs under this lock, so a directory's count and its OS watch never disagree.
struct Watcher {
    backend: notify::RecommendedWatcher,
    /// Handles per canonical directory; the OS watch exists exactly while the count is > 0.
    directories: HashMap<PathBuf, usize>,
    handles: OwnedTable<Arc<Watch>>,
}

/// The OS process's one watcher, made by the first `file/watch`. `None` until then (and
/// after a backend that could not start, which the next watch tries again).
///
/// Lock order: WATCHER, then ROUTES. The backend's own thread takes only ROUTES (in
/// [`on_event`]), and `watch`/`unwatch` on the backend may wait for that thread, so ROUTES
/// is never held across a backend call.
static WATCHER: LazyLock<Mutex<Option<Watcher>>> = LazyLock::new(|| Mutex::new(None));

/// Canonical directory → the handles routed through it. What the backend's thread reads to
/// turn an event into messages.
static ROUTES: LazyLock<Mutex<HashMap<PathBuf, Vec<Arc<Watch>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Start watching `path` for process `owner`: the new handle's ref id, or why not (the
/// directory is missing, or the OS refused a watch).
pub fn watch(path: &str, owner: u64) -> Result<u64, String> {
    let spelled = PathBuf::from(path);
    let absolute = if spelled.is_absolute() {
        spelled.clone()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("the current directory: {error}"))?
            .join(&spelled)
    };
    let (directory, name) = if absolute.is_dir() {
        (absolute, None)
    } else {
        match (absolute.parent(), absolute.file_name()) {
            (Some(parent), Some(name)) => (parent.to_path_buf(), Some(name.to_os_string())),
            _ => return Err(format!("{path}: not a file or directory path")),
        }
    };
    let directory = std::fs::canonicalize(&directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    if !directory.is_dir() {
        return Err(format!("{}: not a directory", directory.display()));
    }
    let mut guard = crate::core::sync::lock(&WATCHER);
    if guard.is_none() {
        let backend = notify::recommended_watcher(on_event)
            .map_err(|error| format!("the operating system refused a file watcher: {error}"))?;
        *guard = Some(Watcher {
            backend,
            directories: HashMap::new(),
            handles: OwnedTable::default(),
        });
    }
    let Some(watcher) = guard.as_mut() else {
        unreachable!("the watcher was just made")
    };
    let count = watcher.directories.get(&directory).copied().unwrap_or(0);
    if count == 0 {
        notify::Watcher::watch(
            &mut watcher.backend,
            &directory,
            notify::RecursiveMode::NonRecursive,
        )
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    }
    watcher.directories.insert(directory.clone(), count + 1);
    let id = crate::process::next_ref();
    let handle = Arc::new(Watch {
        id,
        owner,
        directory: directory.clone(),
        name,
        spelled,
        closed: AtomicBool::new(false),
        queued: Mutex::new(HashSet::new()),
    });
    crate::core::sync::lock(&ROUTES)
        .entry(directory)
        .or_default()
        .push(handle.clone());
    watcher.handles.insert(id, owner, handle);
    Ok(id)
}

/// End handle `id`: true when it was live, false when it had already ended.
pub fn unwatch(id: u64) -> bool {
    let mut guard = crate::core::sync::lock(&WATCHER);
    let Some(watcher) = guard.as_mut() else {
        return false;
    };
    match watcher.handles.remove(&id) {
        Some(handle) => {
            end(watcher, &handle);
            true
        }
        None => false,
    }
}

/// End every handle process `pid` owns — it has exited. A no-op for a process that never
/// watched anything.
pub fn close_process_watches(pid: u64) {
    let mut guard = crate::core::sync::lock(&WATCHER);
    let Some(watcher) = guard.as_mut() else {
        return;
    };
    for (_, handle) in watcher.handles.remove_owned_by(pid) {
        end(watcher, &handle);
    }
}

/// How many live handles watch through `path`'s directory (the path itself when it is a
/// directory): 0 means no OS watch is held there. For tests that prove a watch is released.
pub fn watch_count(path: &str) -> usize {
    let path = Path::new(path);
    let directory = match path.parent() {
        Some(parent) if !path.is_dir() => parent,
        _ => path,
    };
    // A bare file name's parent is the empty path: the current directory.
    let directory = if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        directory
    };
    let Ok(directory) = std::fs::canonicalize(directory) else {
        return 0;
    };
    let guard = crate::core::sync::lock(&WATCHER);
    guard
        .as_ref()
        .and_then(|watcher| watcher.directories.get(&directory).copied())
        .unwrap_or(0)
}

/// Close `handle`, unroute it, and drop the OS watch on its directory if it was the last.
fn end(watcher: &mut Watcher, handle: &Arc<Watch>) {
    handle.closed.store(true, Ordering::Release);
    {
        let mut routes = crate::core::sync::lock(&ROUTES);
        if let Some(handles) = routes.get_mut(&handle.directory) {
            handles.retain(|other| other.id != handle.id);
            if handles.is_empty() {
                routes.remove(&handle.directory);
            }
        }
    }
    let count = watcher
        .directories
        .get(&handle.directory)
        .copied()
        .unwrap_or(0);
    if count <= 1 {
        watcher.directories.remove(&handle.directory);
        // The directory may be gone already, which ended the OS watch with it.
        let _ = notify::Watcher::unwatch(&mut watcher.backend, &handle.directory);
    } else {
        watcher
            .directories
            .insert(handle.directory.clone(), count - 1);
    }
}

/// The paths an event names, each with the change it is to that path.
fn changes(event: &notify::Event) -> Vec<(PathBuf, Change)> {
    use notify::EventKind;
    use notify::event::{ModifyKind, RenameMode};
    let each = |change: Change| {
        event
            .paths
            .iter()
            .map(|path| (path.clone(), change))
            .collect::<Vec<_>>()
    };
    match event.kind {
        EventKind::Access(_) => Vec::new(),
        EventKind::Create(_) => each(Change::Created),
        EventKind::Remove(_) => each(Change::Removed),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => each(Change::Removed),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => each(Change::Created),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            let mut renamed = Vec::new();
            if let Some(from) = event.paths.first() {
                renamed.push((from.clone(), Change::Removed));
            }
            if let Some(to) = event.paths.get(1) {
                renamed.push((to.clone(), Change::Created));
            }
            renamed
        }
        // A rename the backend could not pair (FSEvents reports each side alone): what is
        // there now says which side this path was.
        EventKind::Modify(ModifyKind::Name(_)) => event
            .paths
            .iter()
            .map(|path| {
                let change = if path.exists() {
                    Change::Created
                } else {
                    Change::Removed
                };
                (path.clone(), change)
            })
            .collect(),
        EventKind::Modify(_) | EventKind::Any | EventKind::Other => each(Change::Modified),
    }
}

/// The backend's event handler, on its own thread: route each change to the handles that
/// watch it. An event the backend says it lost track of (a kernel queue overflow) reaches
/// every handle as a change to its own path, so nothing is missed for good.
fn on_event(result: notify::Result<notify::Event>) {
    let Ok(event) = result else {
        return;
    };
    let mut deliveries: Vec<(Arc<Watch>, PathBuf, Change)> = Vec::new();
    {
        let routes = crate::core::sync::lock(&ROUTES);
        if event.need_rescan() {
            for handle in routes.values().flatten() {
                deliveries.push((handle.clone(), handle.spelled.clone(), Change::Modified));
            }
        }
        for (path, change) in changes(&event) {
            route(&routes, &path, change, &mut deliveries);
        }
    }
    for (handle, path, change) in deliveries {
        send(handle, &path, change);
    }
}

/// Collect the handles `path`'s `change` reaches: file watches on its directory that name
/// it, directory watches on its directory (reporting the entry), and directory watches on
/// the path itself (the directory changed or went away).
fn route(
    routes: &HashMap<PathBuf, Vec<Arc<Watch>>>,
    path: &Path,
    change: Change,
    deliveries: &mut Vec<(Arc<Watch>, PathBuf, Change)>,
) {
    if let (Some(directory), Some(name)) = (path.parent(), path.file_name()) {
        for handle in routes.get(directory).into_iter().flatten() {
            match &handle.name {
                Some(watched) if watched == name => {
                    deliveries.push((handle.clone(), handle.spelled.clone(), change))
                }
                Some(_) => {}
                None => deliveries.push((handle.clone(), handle.spelled.join(name), change)),
            }
        }
    }
    for handle in routes.get(path).into_iter().flatten() {
        if handle.name.is_none() {
            deliveries.push((handle.clone(), handle.spelled.clone(), change));
        }
    }
}

/// Deliver `[:file-watch handle path change]` to the handle's owner unless the handle is
/// closed or the same (path, change) is still waiting in the mailbox.
fn send(handle: Arc<Watch>, path: &Path, change: Change) {
    if handle.closed.load(Ordering::Acquire) {
        return;
    }
    let spelled = path.to_string_lossy().into_owned();
    let key = (spelled.clone(), change);
    if !crate::core::sync::lock(&handle.queued).insert(key.clone()) {
        return;
    }
    let message = Message::Vector(vec![
        Message::Keyword(value::intern("file-watch")),
        Message::Ref(handle.id),
        Message::Str(spelled),
        Message::Keyword(value::intern(change.keyword())),
    ]);
    let owner = handle.owner;
    let credit = DeliveryCredit::new(Arc::new(Queued { watch: handle, key }), 1);
    deliver_credited(owner, message, credit);
}
