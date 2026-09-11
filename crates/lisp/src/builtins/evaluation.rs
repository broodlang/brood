//! Evaluating and loading code at runtime: `eval`, `load`, the `%load-string` /
//! `%eval-string` family the REPL and `nest` ride on, `%reload-defs` (hot reload),
//! `%isolate` (the per-file globals scope `nest test` runs each file in), and the
//! debugger's eval-in-paused-context primitives.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::compile::apply_engine;
use crate::syntax::reader;

use super::numeric::{arg, expect_string};
use super::realize_seqview;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    // self-hosting — eval/load/etc. take and return arbitrary forms / values.
    primitives.def(
        "reflect/eval",
        Arity::exact(1),
        Sig::new(vec![any], any),
        &["form"],
        "Evaluate a form in the global environment.",
        eval_builtin,
    );
    primitives.def(
        "reflect/eval-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        &["s"],
        "Read and evaluate every form in string s (the string analogue of load).",
        eval_string,
    );
    primitives.def(
        "%load-string",
        Arity::range(1, 2),
        Sig::new(vec![string, string], any),
        &[],
        "",
        load_string,
    );
    // The embedded-std-module loader: `%load-string` plus the reserved-name
    // exemption, held across the load and released even on a throw (ADR-166).
    primitives.def(
        "%load-module-source",
        Arity::range(1, 2),
        // The second argument is the module's source or nil (`load_module_source` reads
        // the file itself then); `%builtin-module-file` legitimately hands it nil.
        Sig::new(vec![string, string.union(Ty::of(Tag::Nil))], any),
        &[],
        "",
        load_module_source,
    );
    primitives.def(
        "reflect/load",
        Arity::exact(1),
        Sig::new(vec![string], any),
        &["path"],
        "Read and evaluate every form in the file at path.",
        load,
    );
    primitives.def(
        "%run-program-file",
        Arity::exact(1),
        Sig::new(vec![string], any),
        &["path"],
        "Run the program file at `path` as its own green process (ADR-135) and block until it finishes; nil, or raises if a top-level form did. Unlike `load` (which tree-walks inline, so a top-level `receive` blocks the caller), the file runs on a worker in capture mode — top-level `receive`s park-and-capture and message-passing uses the userspace direct-handoff path. Shares this runtime's globals/`*load-path*`. `nest run FILE` routes here.",
        run_program_file);
    primitives.def(
        "system/reload-defs",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Re-evaluate only the def-style top-level forms in `path` (def, defn, defmacro, defmodule, defdyn, …) — skipping other top-level calls. Used by file watchers to refresh code without re-running side-effecting top-level calls like a `(main-loop)`. Returns nil.",
        reload_defs);
    // `apply`'s last positional arg must be a sequence (it's spliced); the
    // intermediate args can be anything. The `Sig` algebra can express
    // "prefix + repeating tail" but not "the *last* item of the tail is
    // special", so the Sig is `(callable, ...any) -> any` — the closest
    // honest approximation. The sequence-at-tail constraint is checked at
    // call time by `apply_builtin` via `heap.seq_items(args[last])`, which
    // surfaces a `wrong_type` error if the last arg isn't a seq. So the
    // Sig is loose, but the runtime is tight.
    primitives.def(
        "apply",
        Arity::at_least(2),
        Sig::with_rest(vec![callable], any, any),
        &["f", "&", "args"],
        "Call f with the leading args plus the final list argument spliced in as trailing args.\n\n    (apply + (list 1 2 3))   → 6",
        apply_builtin,
    );
    primitives.def(
        "%isolate",
        Arity::exact(1),
        Sig::new(vec![callable], any),
        &[],
        "",
        isolate,
    );
    // The quiescence mechanism `%isolate`'s own soundness note asks for: an isolate is sound
    // only while nothing else mutates globals concurrently, and this is how a runner checks
    // before entering one. See `system::scope_live_pids`.
    primitives.def(
        "%scope-live-pids",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        &[],
        "",
        scope_live_pids,
    );
    // The debugger's eval-in-paused-context primitives (ADR-174) — capture a
    // breakpoint's locals and evaluate expressions in that scope. `dev-tools` only.
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%locals",
        Arity::exact(0),
        Sig::new(vec![], any),
        &[],
        "",
        locals,
    );
    // `%scope` is the same intrinsic under a name that reads as "capture the lexical
    // scope"; the VM compiles both to the scope map (see `compile_scope_map`), and this
    // registration is the tree-walker fallback (env-frame read) shared with `%locals`.
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%scope",
        Arity::exact(0),
        Sig::new(vec![], any),
        &[],
        "",
        locals,
    );
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%eval-in",
        Arity::exact(2),
        Sig::new(vec![any, any], any),
        &[],
        "",
        eval_in,
    );
}

pub(super) fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    // Route a runtime-evaluated form through the compiling VM when it's enabled, so a form
    // handed to `eval` isn't stuck on the ~10-14× tree-walker (deferred.md #9): `compile::run`
    // compiles what it can and falls back to the tree-walker per-form for anything outside
    // the VM's vocabulary, so semantics are unchanged; only the top-level call (which
    // dispatches into the VM, where a callee's arm compiles and tail-recurses in O(1) stack)
    // stops being interpreted.
    //
    // The full `compile` pass, matching `reflect/eval-string` and the file loader — so an eval'd
    // form gets namespace resolution, `(:use …)` imports, `(:alias …)`, privacy enforcement
    // and static-quasiquote lowering, and an eval'd `defn` inside a module defines
    // `mod/name` rather than leaking a bare ROOT global.
    //
    // `compile`'s resolve step qualifies a bare reference only on positive evidence, which
    // a file loader supplies by pre-scanning its def heads (`scan_def_names`) — lookahead a
    // one-form-at-a-time `eval` does not have, so a forward reference to a name a LATER
    // `eval` defines was left bare and missed the qualified global (KI-24). `ns_assume_own`
    // supplies the missing conclusion instead of dropping the pass: a bare name bound
    // nowhere is taken to be this namespace's. A no-op at root, where resolve already is.
    let prev_assume = heap.set_ns_assume_own(true);
    let compiled = crate::eval::macros::compile(heap, arg(args, 0), root);
    heap.set_ns_assume_own(prev_assume);
    let form = compiled?;
    crate::eval::compile::run_top_form(heap, form, root)
}

/// `(system/reload-defs path)` — like `load`, but only re-evaluates **definitions**
/// (`def`/`defmacro` and `def…`-named macros: `defn`, `defmodule`, `defdyn`,
/// `defonce`, user definers). All other top-level forms — `(require …)`,
/// `(load …)`, a `(main-loop 0)` entry call — are silently skipped. Used by the
/// file watcher (`std/tool/reload.blsp`): on the **second** and subsequent visits to
/// a file we want to refresh the code (so the running program sees the new
/// behaviour via late binding) but **not** re-run side-effecting top-level calls
/// — re-executing those would spawn a duplicate long-running process (a
/// tail-recursive loop) or block the watcher itself.
///
/// **Atomicity:** the whole file is read before any form is evaluated, so a
/// half-saved / syntactically broken file applies *zero* defs (read fails
/// first). Forms are then expanded+evaluated one at a time, exactly like
/// `load`, so a macro a form defines is visible to later forms in the same file
/// (`lib.rs`). The residual non-atomic window is a *runtime* error while
/// evaluating form N, after 1..N-1 already landed; full snapshot/rollback is
/// deferred (docs/live-editing.md Stage 2). Returns `nil`. ADR-013 hot reload's
/// mechanism flowing through to the tool layer.
pub(super) fn reload_defs(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "system/reload-defs", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("reload-defs: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    // File current BEFORE the read — the reader stamps records with the ambient file;
    // see `load` above (the same ordering bug broke attribution there).
    let prev = heap.set_current_file(Some(path.clone()));
    let forms = match reader::read_all_positioned(heap, &src) {
        Ok(f) => f,
        Err(e) => {
            heap.set_current_file(prev);
            return Err(e.or_file(path.clone()));
        }
    };
    let root = heap.env_root(env);
    // Namespace bracketing + forward-ref pre-scan, like `load` (ADR-065): a reloaded
    // namespaced file re-establishes its own namespace (its `(defmodule …)` form is
    // re-evaluated below) so its re-saved defs are qualified correctly. `NsLoadScope`
    // restores the caller's ns-state on every exit path (incl. panic).
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let mut scope = crate::eval::macros::NsLoadScope::enter(heap, &form_vals);
    let heap = scope.heap();
    let mut result = Ok(Value::nil());
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds; re-fetch
    // each from the (relocated) root stack rather than the stale `forms` Vec. Same
    // discipline as `load`.
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        // Re-eval only *definitions*; skip side-effecting top-level forms
        // (`(require …)`, `(load …)`, a `(main-loop 0)` entry call). A form is a
        // definition when its head symbol starts with "def" **and** is actually a
        // definer — one of the `def`/`defmacro` core special forms, or a symbol
        // currently bound to a macro (`defn`/`defmodule`/`defdyn`/`defonce` and
        // any user `def…` macro). The macro check drops the false positive on a
        // plain top-level *call* to a function whose name merely starts with
        // "def" (e.g. `(default-config)`): that head resolves to a `Fn`, not a
        // macro, so it's correctly skipped.
        //
        // Known limitation (accepted — docs/live-editing.md Stage 2): a definer
        // macro *not* named `def…` (e.g. `(register-handler …)` expanding to a
        // `def`) is skipped. Workaround: prefix definer macros with `def`, the
        // Lisp convention anyway. (`require` skipping is likewise intentional: we
        // don't transitively reload other modules; the user watches each path
        // explicitly with `reload/on-change`.)
        let head_is_def = match form {
            Value::Pair(p) => {
                let (head, _) = heap.pair(p);
                match head {
                    Value::Sym(s) => {
                        let nm = value::symbol_name(s);
                        // The `(defmodule …)` header is re-evaluated too (so the
                        // reloaded file's namespace + imports are re-established for
                        // its defs, ADR-065) — it's a `def…`-named macro, caught here.
                        //
                        // Resolve the head through the current namespace + imports
                        // before the macro check, so a *module-qualified* definer
                        // macro used bare (e.g. `deflive` from `(:use web/live)`,
                        // bound as `web/live/deflive`, not in root) is still
                        // recognised and re-evaluated. Without this, a `(deflive …)`
                        // top-level form would be skipped and its defs never reload.
                        nm.starts_with("def")
                            && (nm == "def" || nm == "defmacro" || {
                                let resolved = crate::eval::macros::resolve_reference(heap, s);
                                matches!(heap.env_get(root, resolved), Some(Value::Macro(_)))
                            })
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !head_is_def {
            continue;
        }
        // Same def-site recording / expand / eval shape as `load` for the
        // forms we *do* evaluate, so cross-file goto still lands at the
        // re-saved def site.
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| {
                // Also record the *expanded* form's def sites, so a macro-defined
                // global whose raw head isn't a def*` (a `defrecord` constructor /
                // accessor, a `defability` op) gets a site too — matching the
                // file-runner (`lib.rs`). Without this, cross-file goto-definition
                // on those names finds nothing under `(load …)` / project modules.
                heap.note_definition(f, pos);
                crate::eval::eval(heap, f, root)
            })
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    result.map(|_| Value::Nil)
    // `scope` drops here → the caller's ns-state is restored (also on panic).
}

pub(super) fn load(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "reflect/load", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("load: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    // The file must be CURRENT before the READ, not just before the eval: the reader
    // stamps every form's position record with `current_file_arc` at read time, so a
    // read-then-set ordering gave every loaded form the CALLER's file (or none) at birth.
    // Invisible for months because the expander's rebuilds re-stamped positions with the
    // by-then-correct ambient file during eval — until ADR-297 made rebuilds copy the
    // original record faithfully, which faithfully preserved the wrong birth record and
    // broke coverage/attribution for loaded modules (`coverage_lines`, 2026-08-29).
    let prev = heap.set_current_file(Some(path.clone()));
    // Read positioned so errors point at a line; tag every error with the file
    // (`FILE:LINE:COL:`, see docs/tooling.md).
    let forms = match reader::read_all_positioned(heap, &src) {
        Ok(f) => f,
        Err(e) => {
            heap.set_current_file(prev);
            return Err(e.or_file(path.clone()));
        }
    };
    let root = heap.env_root(env);
    // A loaded file starts at the ROOT namespace; its own `(defmodule …)` sets the
    // namespace for the rest of the file (ADR-065). `NsLoadScope` resets compile-ns +
    // imports + this file's forward-ref pre-scan + assume-own, and restores the
    // caller's ns-state on EVERY exit path (normal return, `?`, or a panic unwinding
    // through the load) — so ns state never leaks out of a file and the four ns fields
    // stay in sync. It owns the heap for the load; reach it via `scope.heap()`.
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let mut scope = crate::eval::macros::NsLoadScope::enter(heap, &form_vals);
    let heap = scope.heap();

    // **Bounded loading — the core memory guarantee (docs/memory-review.md).**
    // The collector now reclaims at ANY eval depth (ADR-061), so a file loaded
    // here is bounded no matter how deep `(load …)` sits — no `GcBlockReset`
    // depth-1 trick is needed any more. We still root the unevaluated forms across
    // the per-form eval: a collection during form `i` relocates the LOCAL forms
    // `i+1..` this loop still holds, so we re-fetch each from the (relocated) root
    // stack via `root_at` rather than the stale `forms` Vec. (Living in `load`,
    // the core, means every entry path — `brood`, `nest`, MCP `eval`, the future
    // editor — inherits the bound for free.)
    let mut result = Ok(Value::nil());
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| {
                // Also record the *expanded* form's def sites, so a macro-defined
                // global whose raw head isn't a def*` (a `defrecord` constructor /
                // accessor, a `defability` op) gets a site too — matching the
                // file-runner (`lib.rs`). Without this, cross-file goto-definition
                // on those names finds nothing under `(load …)` / project modules.
                heap.note_definition(f, pos);
                crate::eval::eval(heap, f, root)
            })
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    result
    // `scope` drops here → the caller's compile-ns / known-names / imports / assume-own
    // are restored (also on a panic unwinding through the load).
}

/// `(%run-program-file "path")` — run a program **file** as its own green process
/// (ADR-135) and block until it finishes, returning nil (or raising if a top-level form
/// did). Unlike `load` — which tree-walks the file's forms inline, so a top-level
/// `receive` blocks the caller's thread — this drives the file as a real process on a
/// worker in capture mode: a top-level driver talking to a spawned worker uses the
/// userspace direct-handoff path, and top-level `receive`s park-and-capture. It shares
/// this runtime's globals/`*load-path*`, so a preceding `project-setup` (which `def`s the
/// path) is visible to the file's `(require …)`. `nest run FILE` routes here so a run
/// script gets the same fast path as `brood FILE`.
pub(super) fn run_program_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%run-program-file", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("%run-program-file: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    // `false`: this path returns nil/raises, never the printed value — see
    // `ProgramExit::want_result`. `nest run FILE` comes through here, so a run script with a
    // large top-level binding would otherwise render it to a string and drop it.
    let exit = crate::process::spawn_root_program(heap, &src, Some(path.clone()), None, false)
        .map_err(|e| e.or_file(path.clone()))?;
    match exit.wait() {
        Ok(()) => Ok(Value::nil()),
        // The program raised. The error is already file/pos-tagged by the program
        // driver, so render the full report (caret, hint, call trace) and exit 1
        // exactly as `brood FILE` does (run_files) rather than returning an error the
        // caller would re-decorate with a meaningless position inside the generated run
        // script. Restore the terminal first (a TUI that threw before its `term-leave`
        // would otherwise wedge the shell); `process::exit` skips Drop, so do it
        // explicitly — the same no-op-unless-raw call the CLI makes.
        Err(e) => {
            crate::builtins::restore_terminal_on_exit();
            crate::cli_support::report_error(&e);
            std::process::exit(1);
        }
    }
}

/// `(reflect/eval-string "src")` — read and evaluate every form in a string against the
/// global environment. Inherits the current namespace (ADR-065): the REPL evaluates
/// each entry through here, so a `(ns foo)` typed at the REPL sticks to later
/// entries. To load a *module* source at the root namespace, use `%load-string`.
pub(super) fn eval_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "reflect/eval-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, false)
}

/// `(%load-string "src")` / `(%load-string "src" "name")` — the string analogue of
/// `load`: read+eval every form, but bracket the current namespace (reset to root,
/// restore the caller's after), so an embedded module's own `(ns …)` governs it and ns
/// state doesn't leak to the caller. Used by `require-one` for baked-in std modules
/// (ADR-065).
///
/// The optional `name` is what the forms are attributed to for the duration. A baked-in
/// module has no path on disk, and without a name its forms inherit whatever file
/// happened to be loading when the `require` ran — so `std/log`'s lines were reported as
/// the requiring file's, which line coverage caught by crediting a 21-line `main.blsp`
/// with std's line 175. The attribution feeds `CompiledArm::src_file`, hence `:trace`
/// frames too. `require--force` passes the module's real repo-relative path, from
/// `%builtin-module-file` — see [`EmbeddedModule`], which keeps that path in step with
/// the `include_str!` it was baked in from.
pub(super) fn load_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%load-string", arg(args, 0))?;
    let name = match arg(args, 1) {
        Value::Nil => None,
        v => Some(expect_string(heap, "%load-string", v)?),
    };
    let previous_file = name.map(|n| heap.set_current_file(Some(n)));
    let result = eval_string_inner(heap, env, &src, true);
    if let Some(previous) = previous_file {
        heap.set_current_file(previous);
    }
    result
}

/// `(%load-module-source src file)` — load an **embedded std module**'s source with
/// the reserved-name exemption held (ADR-166).
///
/// Identical to `%load-string` except that the module's own `def`s are permitted to
/// (re)bind reserved names *and* become reserved themselves. Two reasons it has to be
/// a primitive rather than a flag `require` sets and clears in Brood: the exemption
/// must be released even when the load **throws** (a leaked one would silently
/// un-reserve the language for the rest of the process's life), and it must not be
/// reachable as an on/off pair that user code could straddle. `require` uses it for
/// baked-in source only — a project file loaded off `*load-path*` goes through the
/// ordinary `load`, so a package's names are never reserved.
pub(super) fn load_module_source(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%load-module-source", arg(args, 0))?;
    let name = match arg(args, 1) {
        Value::Nil => None,
        v => Some(expect_string(heap, "%load-module-source", v)?),
    };
    let previous_file = name.map(|n| heap.set_current_file(Some(n)));
    heap.enter_module_load();
    let result = eval_string_inner(heap, env, &src, true);
    heap.leave_module_load();
    if let Some(previous) = previous_file {
        heap.set_current_file(previous);
    }
    result
}

/// Shared body of `reflect/eval-string` / `%load-string`. When `reset_ns`, the current
/// namespace is reset to root for the duration and the caller's restored after.
pub(super) fn eval_string_inner(
    heap: &mut Heap,
    env: EnvId,
    src: &str,
    reset_ns: bool,
) -> LispResult {
    let root = heap.env_root(env);
    let forms = reader::read_all(heap, src)?;
    // When loading a module (`reset_ns`), bracket the namespace at root and
    // pre-scan its def heads for forward references; the plain `reflect/eval-string` (REPL,
    // inline) inherits the current namespace and does neither (ADR-065).
    let (prev_ns, prev_known, prev_by_module, prev_imports) = if reset_ns {
        let pn = heap.set_compile_ns(None);
        // Region model (ADR-223): per-module pre-scan; active set starts empty and each
        // `defmodule`'s `%in-ns` activates its region (resolution is a no-op at root, so
        // nothing resolves before its region opens).
        let by_module = if crate::eval::macros::file_opens_ns(heap, &forms) {
            crate::eval::macros::scan_regions(heap, &forms)
        } else {
            std::collections::HashMap::new()
        };
        let pk = heap.set_ns_known_names(std::collections::HashSet::new());
        let pbm = heap.set_ns_known_by_module(by_module);
        let pi = heap.set_imports(std::collections::HashMap::new());
        (Some(pn), Some(pk), Some(pbm), Some(pi))
    } else {
        (None, None, None, None)
    };
    // No pre-scan on the inheriting path, so a reference to a name a LATER call will
    // define has no evidence to qualify against and would be left bare, missing the
    // module-qualified global (KI-24) — in the REPL that is just typing `(defmodule m)`
    // and then two mutually recursive `defn`s. Tell the resolver to fall back to the
    // current namespace for a name bound nowhere else; a no-op at root. The module-load
    // path sets it *off*: it has the real pre-scan, and a nested load must not inherit
    // an outer `eval`'s assumption.
    let prev_assume = heap.set_ns_assume_own(!reset_ns);
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds.
    let base = heap.roots_len();
    for &form in &forms {
        heap.push_root(form);
    }
    let mut result: LispResult = Ok(Value::nil());
    for i in 0..forms.len() {
        let form = heap.root_at(base + i);
        // Same as `eval_builtin`/the file loader: compile then run on the VM when enabled
        // (deferred.md #9), tree-walker under `BROOD_VM=0`. `compile::run` falls back to the
        // tree-walker per-form, so a form outside the VM's vocabulary still evaluates.
        match crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::compile::run_top_form(heap, f, root))
        {
            Ok(v) => result = Ok(v),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    heap.truncate_roots(base);
    heap.set_ns_assume_own(prev_assume);
    if let Some(pn) = prev_ns {
        heap.set_compile_ns(pn);
    }
    if let Some(pk) = prev_known {
        heap.set_ns_known_names(pk);
    }
    if let Some(pbm) = prev_by_module {
        heap.set_ns_known_by_module(pbm);
    }
    if let Some(pi) = prev_imports {
        heap.set_imports(pi);
    }
    result
}

/// `(%locals)` / `(%scope)` — the CALLER's in-scope local bindings as a `{:name → value}`
/// map (innermost binding of each name wins; globals excluded). This is the tree-walker
/// fallback: under the VM both spellings are a compiler intrinsic that reads the
/// lexical-scope table directly (see `compile_scope_map`), which is why a compiled arm's
/// call never reaches here. Keyed by the name as a **keyword** to match the intrinsic and
/// the debugger's explicit `:vals`, so a named value overrides a captured local on `merge`
/// and `%eval-in` (which binds keyword- and symbol-keyed entries alike) resolves it.
/// `dev-tools` only (its sole consumer is the `debug` DEV_MODULE).
#[cfg(feature = "dev-tools")]
pub(super) fn locals(_: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let mut seen: Vec<value::Symbol> = Vec::new();
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    let mut cur = Some(env);
    while let Some(e) = cur {
        if e == EnvId::GLOBAL {
            break;
        }
        // Collect this frame's bindings (innermost binding last → scan reversed) into an
        // owned Vec, releasing the borrow before touching `heap` again.
        let (parent, vars) = heap.env_frame_ref(e);
        let frame: Vec<(value::Symbol, Value)> = vars.iter().rev().copied().collect();
        cur = parent;
        for (s, v) in frame {
            if !seen.contains(&s) {
                seen.push(s);
                pairs.push((Value::keyword(s), v));
            }
        }
    }
    Ok(heap.map_from_pairs(pairs))
}

/// `(%eval-in "src" locals-map)` — read + evaluate `src`'s forms in a fresh environment
/// holding `locals-map`'s `{name → value}` bindings over the globals; returns the last
/// result. Lets the debugger evaluate an expression in a paused worker's captured scope,
/// so a breakpoint's locals resolve. GC-safe: a fresh frame per form (used by exactly one
/// `run`, so it can't go stale across a collection); held forms + local values are rooted
/// across the eval. `dev-tools` only.
#[cfg(feature = "dev-tools")]
pub(super) fn eval_in(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%eval-in", arg(args, 0))?;
    let entries: Vec<(value::Symbol, Value)> = match arg(args, 1) {
        Value::Map(mid) => heap
            .map_entries(mid)
            .into_iter()
            .filter_map(|(k, v)| match k {
                Value::Sym(s) | Value::Keyword(s) => Some((s, v)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let forms = reader::read_all(heap, &src)?;
    let fbase = heap.roots_len();
    for &form in &forms {
        heap.push_root(form);
    }
    let lbase = heap.roots_len();
    for &(_, v) in &entries {
        heap.push_root(v);
    }
    let mut result: LispResult = Ok(Value::nil());
    for i in 0..forms.len() {
        let form = heap.root_at(fbase + i);
        let frame = heap.new_env(Some(EnvId::GLOBAL));
        for (j, &(s, _)) in entries.iter().enumerate() {
            let v = heap.root_at(lbase + j);
            heap.env_define(frame, s, v);
        }
        match crate::eval::compile::run(heap, form, frame) {
            Ok(v) => result = Ok(v),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    heap.truncate_roots(fbase);
    result
}

pub(super) fn apply_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity(
            "apply: expected a function and an argument list",
        ));
    }
    // Bind `last` after the guard so the slice indexing below is robust to
    // refactors of the guard: anyone moving / tightening it can't accidentally
    // leave a bare `args[args.len() - 1]` indexing into an empty slice.
    let last = args.len() - 1;
    // The spliced final arg may be a lazy seq-view (`(apply f (map g xs))`) whose
    // realisation re-enters `eval` — a safepoint that can collect and *relocate*
    // LOCAL handles. So the callee `f` and the spliced middle args must be rooted
    // across the realise and re-read after, never trusted as pre-safepoint copies
    // (the re-read discipline ADR-114 requires of any Rust glue holding a LOCAL
    // handle across a GC-capable call; mirrors `prim_eq` / `range_reduce_slow`).
    // Today the only native caller (`%range-reduce` via `apply_value`) never passes
    // a seq-view here, so the realise branch is latent — but the rooting keeps the
    // invariant intact for any future Rust HOF that does.
    heap.root_scope(|heap| {
        let f_r = heap.root(args[0]);
        let mid_roots: Vec<_> = args[1..last].iter().map(|&v| heap.root(v)).collect();
        // `seq_items` can't run a seq-view's transducer, so realise it first.
        let tail = match args[last] {
            sv @ Value::SeqView(_) => realize_seqview(heap, env, sv)?,
            other => other,
        };
        // Re-read across the (possible) collection above before use.
        let f = heap.read_root(f_r);
        let mut argv: Vec<Value> = mid_roots.iter().map(|&r| heap.read_root(r)).collect();
        argv.extend(heap.seq_items(tail)?);
        // Run the target through the active engine (the VM when on), so `apply`-as-a-value
        // — `(map apply …)`, `(reduce apply …)`, apply stored in data — runs its callee
        // compiled, consistent with a direct `(apply f …)` call. This is safe against the
        // `(apply f …)`-driven tail recursion that once forced the tree-walker here
        // (`apply_tail_recursion_does_not_overflow`): a **direct** `apply` call is unfolded
        // by the VM's `dispatch` (it matches the resolved callee, so even `apply` bound to
        // another name unfolds) and TCO'd by the driver, so it never reaches this native;
        // `apply_builtin` is now only hit when a *native* HOF invokes `apply` per element,
        // which loops rather than tail-recurses — one `apply_engine` frame per call, never
        // accumulating. (Deep non-tail recursion in the callee is bounded by the VM's
        // `MAX_BC_FRAMES` guard, not the native stack.)
        apply_engine(heap, f, &argv, env)
    })
}

/// `(%isolate thunk)` — call `thunk` (no args) with a *private copy* of the
/// runtime's global bindings: any `def` it makes is rolled back when it
/// returns, so it cannot affect other code. The test framework wraps each
/// `:isolated` test in this so a test's definitions never leak to another test.
/// Restores the bindings even if the thunk raises (the error then propagates).
///
/// It also reaps processes the thunk left running, so an orphan can't outlive the
/// bindings it was written against. **Blast radius: descendants of this call only.**
/// A process is reaped when it is alive at return, was not alive when the isolate
/// started, and its spawn-ancestry chain reaches a *newly spawned* direct child of
/// this process — i.e. the thunk spawned it, directly or transitively. Bystanders
/// spawned concurrently by anyone else are left alone. The one thing it misses is a
/// grandchild whose intermediate parent already exited (the ancestry link dies with
/// that process), which leaks rather than over-kills — see the filter below.
///
/// **What this does NOT isolate.** Only the global *binding* table is snapshotted
/// and rolled back. Three kinds of leak survive an isolate:
/// - *Memory-only:* the shared code slabs and the symbol interner still grow.
/// - **Behavioural — the `defdyn` mark.** A `defdyn` inside the thunk leaves its
///   name permanently marked dynamic in the process-wide registry
///   (`core::value::DYNAMICS`), even though the binding itself is rolled back. The
///   name is then ambient forever for namespace resolution
///   (`eval::macros::is_ambient`) and permanently exempt from the reserved-name
///   check (`heap::is_sealed` excludes dynamics), so an isolate can silently widen
///   what later code is allowed to define. Fixing this needs an un-mark/restore
///   API on the registry, which it does not currently expose.
/// - **Behavioural — the `private` mark.** A `defn-`/`def-` inside the thunk leaves
///   its qualified name in the runtime's recorded-private set (`RuntimeCode::private`)
///   after the binding is gone, so the now-unbound name still reads as module-private.
///   Same cause: `Heap` exposes `mark_private`/`private_names_snapshot` but no
///   restore.
///
/// (The `sealed` set leaks the same way but benignly — the `in_module_load`
/// exemption means a stale seal never rejects a legitimate def.)
///
/// It is sound only with no other process mutating globals concurrently, which the
/// runner ensures by running isolated tests alone.
/// `(%scope-live-pids)` — the OTHER live processes spawned under the `%isolate` scope this
/// process is currently running in, as a list of pids.
///
/// The mechanism behind "an isolated unit runs alone". `%isolate` is sound only while nothing
/// else mutates globals concurrently — its own doc says so — and the test runner was
/// violating that: an `:isolated` unit enters its isolate and rolls the global table back
/// while the file's parallel workers, and anything they spawned, are still running. A
/// straggler's `defrecord` in that window lands its `%record-register` (locked, so it survives
/// the swap) while the constructor `def` beside it does not, which is KI-89's orphaned
/// record id.
///
/// **Scope, not ancestry.** Every process spawned inside a scope carries it, and so does
/// everything they spawn, however many processes in between have since exited — so this needs
/// no live parent chain, which is the same durability argument as the reap's `isolate_owner`.
///
/// **Scope `0` answers empty, and that is not a shortcut.** Zero is not an ownership group; it
/// is the default bucket every process spawned outside any isolate shares, root and
/// infrastructure included. Counting it asks "is the world quiet?", which is never true — an
/// earlier version did, so the caller's bounded wait timed out every time and cost the suite
/// 5x its wall clock while fixing nothing.
///
/// Policy — how long to wait and what to do when the wait runs out — belongs to the runner, in
/// Brood, not here.
pub(super) fn scope_live_pids(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let scope = crate::process::self_isolate_scope();
    if scope == 0 {
        return Ok(Value::nil());
    }
    let me = crate::process::self_pid();
    // Exclude our own ANCESTORS, and this is the whole difference between a useful answer
    // and a guaranteed timeout. The test runner's shape is root → per-file driver → unit
    // worker, and the driver is parked in `receive` waiting for the very unit that is
    // asking. Counting it says "someone else is live" forever, so a bounded wait on this
    // predicate always ran out — which is exactly what an earlier attempt measured, at 5x
    // the suite's wall clock, and wrongly read as "quiescence is unaffordable".
    //
    // An ancestor cannot be the hazard anyway. The processes that can mutate globals under
    // an isolate are the ones running BESIDE us; the ones we were spawned from are, by
    // construction, blocked on our result.
    let mut ancestors = std::collections::HashSet::new();
    let mut cur = me;
    // Bounded: a parent pid always predates its child so a cycle is impossible, but the
    // walk reads a live table, so cap it rather than trust that.
    for _ in 0..10_000 {
        match crate::process::parent_of(cur) {
            Some(p) => {
                ancestors.insert(p);
                cur = p;
            }
            None => break,
        }
    }
    let items: Vec<Value> = crate::process::list_local_pids()
        .into_iter()
        .filter(|p| {
            *p != me && !ancestors.contains(p) && crate::process::isolate_owner_of(*p) == scope
        })
        .map(crate::process::pid_value)
        .collect();
    Ok(heap.list(items))
}

pub(super) fn isolate(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    // `snapshot_globals`/`restore_globals` now bracket RUNTIME compaction themselves (the
    // snapshot holds off-graph RUNTIME handles a mid-thunk relocation would strand — KI-6),
    // so this reset+run+rollback is compaction-safe with no extra bookkeeping here.
    let saved = heap.snapshot_globals();
    // The global *binding* table is not the whole image. Two mark registries live beside
    // it — the `defdyn` set (`value::DYNAMICS`) and the `defn-` private set — and neither
    // is covered by `snapshot_globals`, so a mark made inside the thunk used to survive
    // the rollback while its binding did not. That is not cosmetic: a leaked dynamic mark
    // makes the name permanently *ambient* for namespace resolution (`macros::is_ambient`,
    // so a later module's `(def *q* …)` stays bare instead of being qualified) and
    // permanently exempt from the reserved-name check (`heap::is_sealed` excludes
    // dynamics). Snapshot both and put them back with the bindings.
    let saved_dynamics = crate::core::value::dynamic_syms();
    let saved_private = heap.private_names_snapshot();
    // Pids alive before the run, to tell apart the ones the thunk spawns.
    let before: std::collections::HashSet<u64> =
        crate::process::list_local_pids().into_iter().collect();
    // Stamp everything the thunk spawns — directly or transitively — with a token unique
    // to this isolate. `spawn` copies the spawner's current scope into the child's
    // `isolate_owner` *and* `isolate_scope`, so the whole subtree carries it from birth
    // and keeps it after any process above it dies. That durability is the entire point:
    // see the reap below.
    let scope = crate::process::next_isolate_token();
    let outer_scope = crate::process::set_self_isolate_scope(scope);
    let result = apply_engine(heap, thunk, &[], env);
    // Restore before the reap, so nothing spawned by the cleanup itself is stamped ours.
    crate::process::set_self_isolate_scope(outer_scope);
    // Reap processes the thunk spawned and left running, BEFORE the wholesale
    // global restore below. Otherwise an orphan still running the test's code (a
    // server it spawned but never stopped) looks up a global the test `def`'d,
    // finds it gone after the swap, and dies with a bogus `unbound symbol` (the
    // flaky-suite race). Kill the newcomers, then **yield** until they deregister
    // — `crate::process::yield_now`, NOT `std::thread::sleep`: this runs inside the
    // isolated unit's own green process, so a thread sleep would freeze its worker
    // and starve any orphan pinned to that same worker. Bounded so a wedged orphan
    // can't hang the run.
    //
    // **The kill set is what the THUNK spawned, by an ownership STAMP — not "alive after
    // minus alive before", and no longer by walking the `parent` chain.** The set
    // difference is not ownership: anything running concurrently — a helper the suite
    // spawned before the isolate, a service under test, a node connection — can spawn
    // during the thunk's run, and the difference swept those bystanders in and killed them.
    //
    // The ancestry walk that replaced it was correct but not total, and its gap is KI-89.
    // A chain that dead-ends at a process which has already exited (its registry entry, and
    // with it its own parent link, is gone) could not be proven ours, so an orphaned
    // GRANDCHILD whose middle process exited during the thunk was left running. That is not
    // a mere process leak: the orphan keeps executing against the globals we are about to
    // roll back, and a `require`-driven `defrecord` in it lands its `%record-register`
    // (locked, so it survives the swap) while the constructor `def` beside it goes to the
    // table the swap discards — a registered record id with no bound constructor, sticky
    // from the next snapshot on. It was observed exactly that way: seven ids written in one
    // burst by a grandchild, between two of the runner's restores.
    //
    // `isolate_owner` closes it. It is stamped at spawn from the spawner's current scope
    // and never mutated, so it survives every death above it and needs no live chain to
    // read. A bystander carries some other token (or none) and is still left alone, so the
    // blast radius above is unchanged — it is only the missing half that is added.
    let me = crate::process::self_pid();
    let spawned: std::collections::HashSet<u64> = crate::process::list_local_pids()
        .into_iter()
        // Never reap the CALLER: the root's mailbox registers lazily (its first
        // `receive`), so a root that had never received before this isolate ran
        // shows up as a "newcomer" — and the reap would exit-kill the very process
        // running the isolate. That kill was silently ignored for as long as
        // exit signals couldn't reach a natively-nested receive; now that they can
        // (Control::Killed), it would abort the whole run.
        .filter(|p| {
            !before.contains(p) && *p != me && crate::process::isolate_owner_of(*p) == scope
        })
        .collect();
    if !spawned.is_empty() {
        let kill = crate::process::Message::Keyword(crate::core::value::intern(
            crate::process::keywords::KILL,
        ));
        for &pid in &spawned {
            // Unlink the child from THIS isolate runner before killing it. A child the
            // thunk `spawn-link`ed is symmetrically linked to us, so a bare
            // `(exit pid :kill)` would propagate `:killed` back through the link and
            // kill the runner itself — even though we're only cleaning up leftovers.
            // Dropping the link first lets the reap take down any straggler (e.g. a
            // server whose async `(stop …)` hasn't finished dying yet) without taking us
            // with it. Best-effort + a no-op for an unlinked child. (Fixes a capture-mode
            // flake where the stop-vs-reap race left a linked server alive at reap; §8.4.)
            crate::process::unlink_self(pid);
            crate::process::exit(pid, kill.clone());
        }
        // The join must WAIT, not spin. `yield_now` is `std::thread::yield_now` — a hint
        // the OS is free to ignore — so the old `for _ in 0..10_000 { yield }` could burn
        // through in microseconds while a parked victim's kill still needed a scheduler
        // worker to process it, and the "join" returned with the corpses mid-death. On the
        // ROOT thread (where `brood --test` runs `:isolated` units) that was deterministic:
        // `tests/remote_spawn_test.blsp` failed 4/4 because the reaped `:remote-spawn`
        // server was still name-registered when the next test's `serve-spawns` checked —
        // it saw "already serving", declined to restart, and every spawn request went to
        // the corpse. So: yield briefly (cheap when the deaths are already processed),
        // then back off to real micro-sleeps under a wall-clock bound. A short sleep on a
        // green worker is acceptable — the victims retire on OTHER workers; the one victim
        // a sleep cannot help (pinned to THIS worker) was equally beyond the spin loop,
        // and the bound keeps a wedged orphan from hanging the run either way.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut spins = 0u32;
        loop {
            if !crate::process::list_local_pids()
                .into_iter()
                .any(|p| spawned.contains(&p))
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break; // give up on a wedged orphan; the bound is the point
            }
            if spins < 64 {
                spins += 1;
                crate::process::yield_now();
            } else {
                std::thread::sleep(std::time::Duration::from_micros(500));
            }
        }
    }
    // DIAGNOSTIC (BROOD_SCOPE_DBG): what is still alive as we roll the globals back. Anything
    // here that is not an ancestor is a process about to see its bindings vanish — KI-89.
    if std::env::var_os("BROOD_SCOPE_DBG").is_some() {
        let mut ancestors = std::collections::HashSet::new();
        let mut cur = me;
        for _ in 0..10_000 {
            match crate::process::parent_of(cur) {
                Some(p) => {
                    ancestors.insert(p);
                    cur = p;
                }
                None => break,
            }
        }
        let survivors: Vec<String> = crate::process::list_local_pids()
            .into_iter()
            .filter(|p| *p != me && !ancestors.contains(p))
            .map(|p| {
                format!(
                    "{p}(scope={},parent={:?})",
                    crate::process::isolate_owner_of(p),
                    crate::process::parent_of(p)
                )
            })
            .collect();
        if !survivors.is_empty() {
            eprintln!(
                "[scope] RESTORE by {me} (scope={scope}) with {} live: {}",
                survivors.len(),
                survivors.join(" ")
            );
        }
    }
    heap.restore_globals(saved);
    // Both registries are restored on the error path too — `result` is returned below
    // rather than `?`-propagated, so a throwing thunk rolls back exactly as a clean one
    // does. Replaces rather than unions, so a mark *added* inside the thunk is dropped.
    crate::core::value::restore_dynamics(saved_dynamics);
    heap.restore_private_names(saved_private);
    result
}
