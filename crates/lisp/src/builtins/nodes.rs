//! Distributed nodes (ADR-033/034): listen, connect, name the local node, monitor and
//! disconnect peers. `std/node.blsp` is the policy over these `%node-*` primitives.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_string};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // distributed nodes (connect two runtimes over TCP — crate::dist)
    primitives.def(
        "%node-listen",
        Arity::exact(3),
        // The node name may be a symbol OR a keyword — `expect_node_name` accepts both, and
        // the prelude's `node/start` passes the computed `:name@host` keyword (matching
        // `%node-connect`). Returns the qualified node name, always a keyword.
        Sig::new(vec![sym.union(kw), string, string], kw),
        &[],
        "",
        node_listen,
    );
    primitives.def(
        "%node-also-listen",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &[],
        "",
        node_also_listen,
    );
    primitives.def(
        "%node-connect",
        Arity::exact(2),
        // The peer name may be a symbol OR a keyword — `expect_node_name` accepts
        // both, and the prelude's `connect` passes the computed `:name@host`
        // keyword (matches `register`/`proc/whereis`/`monitor-node`). Returns the
        // authoritative peer name, always as a keyword (`Value::keyword`).
        Sig::new(vec![sym.union(kw), string], kw),
        &[],
        "",
        node_connect,
    );
    primitives.def(
        "file/spit-private",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        &["path", "s"],
        "Write string s to path with owner-only (0600) permissions, creating the parent dir if needed. The private-by-default write for a secret (file/spit leaves a world-readable file).\n\n    (file/spit-private \"secrets/token\" \"s3cr3t\")   ;~> nil, the file readable by its owner only",
        super::filesystem::watched!("file/spit-private", spit_private, 1));
    // `node/name` is the keyword `:nonode` until `node/start` sets it to a symbol.
    primitives.def(
        "%node-name",
        Arity::exact(0),
        Sig::nullary(sym.union(kw)),
        &[],
        "This runtime's node name (:nonode until node/start).",
        node_name,
    );
    primitives.def(
        "%nodes",
        Arity::exact(0),
        Sig::nullary(list_ty),
        &[],
        "A list of currently connected peer node names.",
        nodes,
    );
    primitives.def(
        "%monitor-node",
        Arity::exact(1),
        // A node name may be a symbol OR a keyword — `node-name`/`connect` return
        // the authoritative `:name@host` as a keyword, so monitoring it must not
        // warn (matches `register`/`proc/whereis`; `expect_node_name` accepts both).
        Sig::new(vec![sym.union(kw)], kw),
        &["name"],
        "Get [:nodedown name] when the link to node `name` goes down (heartbeat timeout or close). Returns the node's name as a keyword.",
        monitor_node,
    );
    primitives.def(
        "%demonitor-node",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw)], nil_ty),
        &["name"],
        "Cancel this process's node monitor for node `name` (undo node/monitor); a no-op if none is registered. Returns nil.",
        demonitor_node);
    primitives.def(
        "%disconnect",
        Arity::exact(1),
        // Same name domain as `monitor-node`: the authoritative `:name@host`
        // keyword `connect`/`nodes` hand back (or a symbol).
        Sig::new(vec![sym.union(kw)], bool_ty),
        &["name"],
        "Tear down the link to peer node `name` now, without exiting this process — fires [:nodedown name] on both sides and prunes `name` from (nodes). Returns true if a link existed, false otherwise. Use it to leave a node/cluster cleanly while staying alive.",
        disconnect);
    // guest links (ADR-415) — `std/node.blsp`'s grant-guest / revoke-guest / peer-grant
    primitives.def(
        "%node-grant-guest",
        Arity::exact(3),
        Sig::new(vec![string, any, any], nil_ty),
        &["cookie", "names", "as"],
        "Accept peers that authenticate with `cookie` as guests: their link reaches only the registered `names` (a list/vector of keywords) and the pids this node has sent them, and a message from it carrying a function is dropped. `as` (a string, or nil) is who the guest is, read back with %node-peer-grant. The cookie must be at least 16 characters. Replaces an earlier grant of the same cookie.",
        node_grant_guest);
    primitives.def(
        "%node-revoke-guest",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        &["cookie"],
        "Stop accepting `cookie` from new guests; true when it was granted. A guest already connected under it stays until disconnected.",
        node_revoke_guest);
    primitives.def(
        "%node-peer-grant",
        Arity::exact(1),
        Sig::new(vec![any], map_ty.union(nil_ty)),
        &["peer"],
        "The grant a connected peer authenticated with — `{:names [...] :as ... :claimed ...}`, `:claimed` being the node name the guest gave itself (its name here is one this node minted) — or nil for a full peer or an unknown one. `peer` is a node name or a pid on that node.",
        node_peer_grant);
}

/// Coerce a node/name argument (a keyword or symbol) to its interned `Symbol`.
/// Goes through the same `wrong_type` formatter as the other `expect_*`
/// helpers, so the message names the offending value.

pub(super) fn expect_node_name(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "keyword or symbol",
        Value::Keyword(s) => s,
        Value::Sym(s) => s,
    )
}

/// `(node-start name "host:port" cookie)` — name this runtime and listen for peer
/// nodes. Returns the node name.
/// `(%node-listen name addr cookie)` — the listen mechanism behind the prelude's
/// `node-start`. `addr` carries the transport (`"unix:PATH"` / `"tcp:HOST:PORT"`);
/// the path/cookie/transport policy lives in `std/prelude.blsp` (ADR-068).
pub(super) fn node_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "%node-listen", arg(args, 0))?;
    let addr = expect_string(heap, "%node-listen", arg(args, 1))?;
    let cookie = expect_string(heap, "%node-listen", arg(args, 2))?;
    crate::dist::node_listen(name, &addr, cookie).map_err(|e| {
        LispError::runtime(format!("node/start: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::keyword(name))
}

/// `(%node-also-listen addr)` — add another listener to an already-started node
/// (dual-listen, ADR-074). `addr` carries the transport (`"unix:PATH"` /
/// `"tcp:HOST:PORT"`); shares the node's existing identity + cookie.
pub(super) fn node_also_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let addr = expect_string(heap, "%node-also-listen", arg(args, 0))?;
    crate::dist::node_also_listen(&addr).map_err(|e| {
        LispError::runtime(format!("%node-also-listen: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::nil())
}

/// `(%node-connect peer addr)` — the dial mechanism behind the prelude's
/// `connect`. `peer` is the expected node name (self-guard + de-dup); `addr`
/// carries the transport. Returns the peer's authoritative node name.
pub(super) fn node_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let peer = expect_node_name(heap, "%node-connect", arg(args, 0))?;
    let addr = expect_string(heap, "%node-connect", arg(args, 1))?;
    let real = crate::dist::node_connect(peer, &addr).map_err(|e| {
        LispError::runtime(format!("%node-connect: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::keyword(real))
}

/// `(spit-private path s)` — write `s` to `path` with owner-only (`0600`)
/// permissions, creating the parent directory if needed. The private-by-default
/// write a secret needs (`spit` leaves a world-readable file); the cookie-file
/// policy that uses it is Brood (`node-cookie`, ADR-068).
pub(super) fn spit_private(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write as _;
    let path = expect_string(heap, "file/spit-private", arg(args, 0))?;
    let content = expect_string(heap, "file/spit-private", arg(args, 1))?;
    let err = |e: std::io::Error| {
        LispError::runtime(format!("file/spit-private: {path}: {e}"))
            .with_code(crate::error::error_codes::FILE_IO)
    };
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    // Owner-only: 0600 on Unix, a one-entry DACL on Windows (below).
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(err)?;
        // `.mode` only applies on *create*; enforce 0600 on a pre-existing file too.
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        f.write_all(content.as_bytes()).map_err(err)?;
    }
    // Windows: the file is CREATED with a DACL granting only the current user, protected
    // from its directory's inherited entries — owner-only from its first moment, as 0600 is
    // (KI-365); a file already there is made owner-only before it is emptied.
    #[cfg(windows)]
    {
        let mut f = crate::host::owner_only::create_private_file(std::path::Path::new(&path))
            .map_err(err)?;
        f.write_all(content.as_bytes()).map_err(err)?;
    }
    // wasm has no filesystem permissions: a plain private-intent write.
    #[cfg(not(any(unix, windows)))]
    {
        let mut f = std::fs::File::create(&path).map_err(err)?;
        f.write_all(content.as_bytes()).map_err(err)?;
    }
    Ok(Value::nil())
}

/// `(node-name)` — this runtime's node name (`:nonode` until `node-start`).
pub(super) fn node_name(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::keyword(crate::dist::local_node()))
}

/// `(monitor-node name)` — the calling process is sent `[:nodedown name]` when a
/// link to `name` goes down (heartbeat timeout or clean close). Returns the name.
pub(super) fn monitor_node(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "%monitor-node", arg(args, 0))?;
    crate::dist::monitor_node(name, crate::process::self_pid());
    Ok(Value::keyword(name))
}

/// `(demonitor-node name)` — cancel the calling process's node monitor for `name`.
/// A no-op if no monitor is registered. Returns `nil`.
pub(super) fn demonitor_node(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "%demonitor-node", arg(args, 0))?;
    crate::dist::demonitor_node(name, crate::process::self_pid());
    Ok(Value::nil())
}

/// `(disconnect name)` — drop the link to peer `name` now, without exiting
/// this process. Returns `true` if a link existed, `false` otherwise.
pub(super) fn disconnect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "%disconnect", arg(args, 0))?;
    Ok(Value::boolean(crate::dist::disconnect(name)))
}

/// `(nodes)` — a list of currently connected peer node names.
pub(super) fn nodes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let names: Vec<Value> = crate::dist::connected_nodes()
        .into_iter()
        .map(Value::Keyword)
        .collect();
    Ok(heap.list(names))
}

// ----- guest grants (ADR-415) -------------------------------------------------

/// The shortest cookie a guest grant takes. A grant is handed to someone else, often over
/// a chat message, so it must not be guessable — and a short one is a typo away from a
/// password someone chose.
const MIN_GUEST_COOKIE_LEN: usize = 16;

/// `(%node-grant-guest cookie names as)` — accept peers that authenticate with `cookie` as
/// guests reaching `names` (registered names) and identified as `as` (a string, or nil).
pub(super) fn node_grant_guest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let cookie = expect_string(heap, "%node-grant-guest", arg(args, 0))?;
    if cookie.chars().count() < MIN_GUEST_COOKIE_LEN {
        return Err(LispError::runtime(format!(
            "node/grant-guest: a guest cookie needs at least {MIN_GUEST_COOKIE_LEN} characters"
        )));
    }
    let names = heap
        .seq_items(arg(args, 1))?
        .into_iter()
        .map(|name| expect_node_name(heap, "%node-grant-guest", name))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    let as_name = match arg(args, 2) {
        Value::Nil => None,
        other => Some(expect_string(heap, "%node-grant-guest", other)?),
    };
    crate::dist::guest::grant(cookie, names, as_name);
    Ok(Value::nil())
}

/// `(%node-revoke-guest cookie)` — forget a grant; true when there was one.
pub(super) fn node_revoke_guest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let cookie = expect_string(heap, "%node-revoke-guest", arg(args, 0))?;
    Ok(Value::boolean(crate::dist::guest::revoke(&cookie)))
}

/// `(%node-peer-grant peer)` — the grant a connected peer (a node name, or a pid on that
/// node) authenticated with, as `{:names [...] :as ... :claimed ...}`, or nil for a full peer.
/// `:claimed` is the node name the guest gave itself, which is never its name here (ADR-425).
pub(super) fn node_peer_grant(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let node = match arg(args, 0) {
        Value::Pid { node, .. } => node,
        other => expect_node_name(heap, "%node-peer-grant", other)?,
    };
    let Some(link) = crate::dist::peer_grant(node) else {
        return Ok(Value::nil());
    };
    let grant = link.grant();
    let mut names: Vec<Value> = grant.names.iter().copied().map(Value::Keyword).collect();
    names.sort_by(|a, b| {
        let name_of = |v: &Value| match v {
            Value::Keyword(s) => value::symbol_name(*s).to_string(),
            _ => String::new(),
        };
        name_of(a).cmp(&name_of(b))
    });
    let names = heap.alloc_vector(names);
    let as_value = match &grant.as_name {
        Some(name) => heap.alloc_string(name),
        None => Value::nil(),
    };
    Ok(heap.map_from_pairs(vec![
        (Value::keyword(value::intern("names")), names),
        (Value::keyword(value::intern("as")), as_value),
        (
            Value::keyword(value::intern("claimed")),
            Value::keyword(link.claimed()),
        ),
    ]))
}
