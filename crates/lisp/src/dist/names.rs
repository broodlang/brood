//! The local registered-name table (`proc/register`, `proc/whereis`, `proc/unregister`),
//! its death-path sweep, and named-spawn's atomic check-or-spawn — the names a peer can
//! address a process by before it holds the pid.

use super::*;

/// Locally registered name → local process id, so a peer can address a process by
/// a stable name before anyone holds its pid (`(proc/register :echo (self))`).
pub(super) static NAMES: LazyLock<RwLock<NameTable>> =
    LazyLock::new(|| RwLock::new(NameTable::default()));

/// The registered names, indexed both ways. `by_pid` exists for the death path: every
/// process exit sweeps its names, and with only `name → pid` that sweep was a pass over
/// EVERY registered name, under the write lock, per exit. Both maps change together
/// under the one `NAMES` lock, through the methods below and nowhere else, so each is
/// always the other's inverse.
#[derive(Default)]
pub(super) struct NameTable {
    by_name: HashMap<Symbol, u64>,
    /// pid → the names bound to it, in binding order. Nearly always one name or none (an
    /// entry is removed when its last name goes).
    by_pid: HashMap<u64, Vec<Symbol>>,
}

impl NameTable {
    pub(super) fn get(&self, name: Symbol) -> Option<u64> {
        self.by_name.get(&name).copied()
    }

    /// Bind `name` to `pid`, taking it from any previous holder.
    fn bind(&mut self, name: Symbol, pid: u64) {
        match self.by_name.insert(name, pid) {
            Some(previous) if previous == pid => return,
            Some(previous) => self.forget_name_of(previous, name),
            None => {}
        }
        self.by_pid.entry(pid).or_default().push(name);
    }

    /// Drop `name`'s binding, returning the pid it named.
    fn unbind(&mut self, name: Symbol) -> Option<u64> {
        let pid = self.by_name.remove(&name)?;
        self.forget_name_of(pid, name);
        Some(pid)
    }

    /// Drop every name bound to `pid`.
    fn unbind_pid(&mut self, pid: u64) {
        for name in self.by_pid.remove(&pid).unwrap_or_default() {
            self.by_name.remove(&name);
        }
    }

    fn holds_any(&self, pid: u64) -> bool {
        self.by_pid.contains_key(&pid)
    }

    /// The first name still bound to `pid`.
    fn first_name_of(&self, pid: u64) -> Option<Symbol> {
        self.by_pid
            .get(&pid)
            .and_then(|names| names.first().copied())
    }

    /// Remove `name` from `pid`'s reverse entry (the forward one is the caller's).
    fn forget_name_of(&mut self, pid: u64, name: Symbol) {
        if let Some(names) = self.by_pid.get_mut(&pid) {
            names.retain(|&held| held != name);
            if names.is_empty() {
                self.by_pid.remove(&pid);
            }
        }
    }
}

/// What [`register`] did.
pub(crate) enum Registration {
    /// `name` now names the pid, replacing any previous holder.
    Bound,
    /// The pid is not alive, so nothing was bound. Indistinguishable from binding it and
    /// the process dying a moment later — a name lives only as long as its process, so the
    /// death would have released it — which is why this is not an error: a supervisor that
    /// registers a child which crashed on its first line must not crash with it.
    DeadPid,
}

/// `(proc/register name pid)` — bind a local name to a local process id, replacing any
/// previous holder. Replacing is the contract every caller relies on (a restarted
/// logger, a test standing in as `:editor` and handing the name back), so a live
/// holder is not refused.
///
/// Decided under the `NAMES` write lock, against the same lock the death path holds
/// across its name sweep AND its `REGISTRY` removal ([`unregister_dead_pid_while`]), so
/// the liveness read here is exact: a pid this sees alive has not yet been swept, and
/// the sweep will find the binding. Without the check a dead pid's name was bound after
/// its sweep and stayed bound forever.
pub(crate) fn register(name: Symbol, id: u64) -> Registration {
    let mut names = crate::core::sync::write(&NAMES);
    if !process::is_alive(id) {
        return Registration::DeadPid;
    }
    names.bind(name, id);
    Registration::Bound
}

/// `(proc/whereis name)` — the local pid registered under `name`, or `None`. Lets
/// callers test for an existing registration before re-`spawn`ing a server
/// they're about to register (idempotent bootstrap; used by `remote-spawn`).
pub(crate) fn whereis(name: Symbol) -> Option<u64> {
    crate::core::sync::read(&NAMES).get(name)
}

/// `(proc/unregister name)` — drop `name`'s binding, returning whether one existed.
///
/// The inverse of [`register`], which had none: a name could be bound to a pid and never
/// released except by that process dying. A service that wants to hand its name to a
/// replacement, or step down without exiting, needs this.
pub(crate) fn unregister(name: Symbol) -> bool {
    crate::core::sync::write(&NAMES).unbind(name).is_some()
}

/// The name `pid` is registered under, if any — the reverse of [`whereis`].
/// Used by the scheduler's death reporter to name a crashed process
/// (`process ticker (pid 6) died: …`) instead of only its opaque pid. With several
/// names bound, the first still held. Must be read *before*
/// [`unregister_dead_pid_while`] clears the entry.
pub(crate) fn name_for_pid(pid: u64) -> Option<Symbol> {
    crate::core::sync::read(&NAMES).first_name_of(pid)
}

/// The death path's name sweep: remove every `NAMES` entry pointing at `pid` (a name
/// lives only as long as its process — else `whereis` returns a corpse and named-spawn
/// never respawns), and prune `pid` from every `NODE_MONITORS` watcher list.
///
/// `retire` (the `REGISTRY` removal) runs **inside** the `NAMES` lock with the sweep, so
/// no [`register`] can fall between them, see the pid still alive, and bind a name the
/// sweep has already passed — and a pid absent from `REGISTRY` is already absent from
/// `NAMES` (KI-80). NAMES → REGISTRY is the nesting [`spawn_or_get`] and [`register`]
/// already use, so this adds no new lock order.
///
/// A pid holding no name (nearly every one) retires under the READ lock, so exits do not
/// serialise on the table. That keeps both properties, because every binding is made
/// under the WRITE lock, which a held read lock excludes: a [`register`] that saw the pid
/// alive finished before this read and left a binding it sees (the write path below), and
/// one that runs after the read is released finds the pid already gone from `REGISTRY`.
pub(crate) fn unregister_dead_pid_while<R>(pid: u64, retire: impl FnOnce() -> R) -> R {
    let retired = {
        let names = crate::core::sync::read(&NAMES);
        if names.holds_any(pid) {
            drop(names);
            // Anything bound between the two locks is swept too: the write lock holds
            // off every further `register` until `retire` has run.
            let mut names = crate::core::sync::write(&NAMES);
            names.unbind_pid(pid);
            retire()
        } else {
            retire()
        }
    };
    // Prune the dead pid from every NODE_MONITORS watcher list — when it is on one. The
    // read is the sweep's linearisation point when it is not: a `monitor_node` naming
    // this pid after it is a registration after the sweep, which the unconditional write
    // lock admitted just the same (`monitor_node` does not check liveness).
    let watching = crate::core::sync::read(&NODE_MONITORS)
        .values()
        .any(|watchers| watchers.contains(&pid));
    if watching {
        let mut monitors = crate::core::sync::write(&NODE_MONITORS);
        for watchers in monitors.values_mut() {
            watchers.retain(|&w| w != pid);
        }
        monitors.retain(|_, v| !v.is_empty());
    }
    retired
}

/// Named-spawn's atomic check-or-spawn primitive. If `name` is registered
/// to a still-alive pid, return that pid and skip the spawn. Otherwise,
/// drop any stale entry, call `spawner` to create a fresh process, register
/// it under `name`, and return the new pid.
///
/// The whole sequence runs under the `NAMES` write lock so two concurrent
/// `(spawn :name …)` calls can't both spawn — the loser sees the winner's
/// pid and returns it. Inside, REGISTRY is briefly acquired **twice**:
/// once via `process::is_alive` for the staleness check, and once inside
/// `spawner()` (`process::spawn` inserts a new mailbox). Both are short
/// — sequential acquisitions, not held across awaits, never overlap with
/// each other. Lock-ordering vs `deregister` is safe: deregister nests its
/// REGISTRY removal INSIDE NAMES ([`unregister_dead_pid_while`]) — the same
/// NAMES → REGISTRY order as here — and never holds REGISTRY while reaching
/// for NAMES, so no cycle can form.
///
/// `spawner` is **fallible** — if creating the process errors (e.g. a
/// type-check or heap-promotion failure), we propagate without inserting
/// into NAMES, so a failed spawn leaves no stale entry behind.
pub(crate) fn spawn_or_get<E>(
    name: Symbol,
    spawner: impl FnOnce() -> Result<u64, E>,
) -> Result<u64, E> {
    let mut names = crate::core::sync::write(&NAMES);
    if let Some(existing) = names.get(name) {
        if process::is_alive(existing) {
            return Ok(existing);
        }
        // Stale (the process registered under this name has died); drop and
        // fall through to a fresh spawn.
        names.unbind(name);
    }
    let pid = spawner()?;
    names.bind(name, pid);
    Ok(pid)
}
