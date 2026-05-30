//! Introspection queries answered by a project `Interp`. Asks the *language's*
//! globals (prelude + builtins + anything the project has loaded) — completion
//! candidates, the arglist + docstring of a name under the cursor, etc. They
//! run by `eval`ing the introspection primitives (`global-names` / `arglist` /
//! `doc`, ADR-025), never user buffer text.
//!
//! Two clients consume this: `brood-lsp` (hover/completion/signature, never
//! evaluates a buffer — only these queries) and the planned `nest mcp`
//! (the agent-side counterpart, ADR-036 / `docs/mcp.md`). Keeping the surface
//! here ensures the two cannot drift on what `map`'s signature is.
//!
//! **Contract for every operation in this module:**
//!  1. **Total** — failures become typed-`None`/empty results, never panics.
//!  2. **LOCAL-clean** — reclaim allocations with `Heap::checkpoint` /
//!     `reset_local_to` before returning. A long-running tooling session must
//!     not leak a fresh list per query.

use crate::core::heap::Heap;
use crate::core::value::{self, Value};
use crate::error::Pos;
use crate::Interp;

/// Every global the interpreter knows (prelude + builtins), sorted by spelling
/// (`global-names` sorts) — the completion candidate pool.
pub fn global_names(interp: &mut Interp) -> Vec<String> {
    // The result is a list of *every* global (hundreds of symbols), and this
    // runs on every completion keystroke. `eval_str` keeps its result in LOCAL,
    // so reclaim it once we've copied the names into owned `String`s — otherwise
    // a long server session leaks a fresh list per request. (Safe: `symbol_name`
    // reads the interner, not LOCAL, so `names` holds no LOCAL handle.)
    let cp = interp.heap.checkpoint();
    let names = match interp.eval_str("(global-names)") {
        Ok(v) => interp
            .heap
            .list_to_vec(v)
            .map(|items| {
                items
                    .into_iter()
                    .filter_map(|x| match x {
                        Value::Sym(s) => Some(value::symbol_name(s)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    interp.heap.reset_local_to(cp);
    names
}

/// The `(signature, docstring)` of a global `name`, via `(list (arglist NAME)
/// (doc NAME))`. `name` is a CST symbol token, so it can't contain a delimiter,
/// comment, or quote char (see [`is_delimiter`]) — interpolating it can't escape
/// the expression. Returns `(None, None)` when `name` is unbound (eval errors)
/// or names something without an arglist/doc.
///
/// [`is_delimiter`]: crate::syntax::atom::is_delimiter
pub fn signature(interp: &mut Interp, name: &str) -> (Option<String>, Option<String>) {
    // As in `global_names`: reclaim the LOCAL allocations this eval leaves behind
    // once the signature/doc have been copied into owned `String`s.
    let cp = interp.heap.checkpoint();
    let out = match interp.eval_str(&format!("(list (arglist {name}) (doc {name}))")) {
        Ok(v) => match interp.heap.list_to_vec(v) {
            Ok(parts) => {
                let sig = parts.first().and_then(|&a| render_arglist(interp, name, a));
                let doc = match parts.get(1) {
                    Some(&Value::Str(id)) => Some(interp.heap.string(id).to_string()),
                    _ => None,
                };
                (sig, doc)
            }
            Err(_) => (None, None),
        },
        Err(_) => (None, None),
    };
    interp.heap.reset_local_to(cp);
    out
}

/// The raw parameter tokens of a global function/macro `name` — the names *and*
/// the `&optional` / `&` markers, in source order (e.g. `["a", "&optional", "b",
/// "&", "rest"]`). `None` when `name` is unbound or has no params (a builtin or a
/// zero-arg fn — indistinguishable here, as in [`signature`]). For signature help.
pub fn arglist_tokens(interp: &mut Interp, name: &str) -> Option<Vec<String>> {
    let cp = interp.heap.checkpoint();
    let out = match interp.eval_str(&format!("(arglist {name})")) {
        Ok(v) => interp.heap.list_to_vec(v).ok().map(|items| {
            items
                .into_iter()
                .filter_map(|x| match x {
                    Value::Sym(s) => Some(value::symbol_name(s)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        }),
        Err(_) => None,
    };
    interp.heap.reset_local_to(cp);
    out.filter(|v| !v.is_empty())
}

/// Render an `(arglist f)` result (a list of parameter symbols, or `nil`) as a
/// signature line `(name p1 p2 …)`. An empty result (a builtin, a non-fn, *or* a
/// zero-arg fn) yields `None`: we can't tell a zero-arg fn from a non-fn here, so
/// we show no signature rather than a misleading one.
fn render_arglist(interp: &Interp, name: &str, arglist: Value) -> Option<String> {
    let items = interp.heap.list_to_vec(arglist).ok()?;
    if items.is_empty() {
        return None;
    }
    let mut s = format!("({name}");
    for it in items {
        if let Value::Sym(p) = it {
            s.push(' ');
            s.push_str(&value::symbol_name(p));
        }
    }
    s.push(')');
    Some(s)
}

// ============================================================================
// Step 1b — wider tooling surface for the planned `nest mcp` (ADR-036). Each
// operation below holds the same contract as the introspection helpers above:
// total (errors become typed result fields, not panics) and LOCAL-clean
// (every `eval_str` is bracketed by `checkpoint` / `reset_local_to`, so a
// long-running session does not leak a fresh list per call).
//
// **Two MCP tools are not yet wrappable here** and are deliberately deferred:
//   * `check_project` — the Brood-side `(check-project)` is print-oriented
//     (GNU lines to stdout + an `Int` count). A structured variant in
//     `std/project.blsp` (returning `[file line col message]` tuples) is the
//     right shape and lands with step 2 of the MCP plan, not here.
//   * `run_tests`     — same issue: `(run-project-tests)` prints per-test
//     GNU output and raises on failure. Needs a structured runner result.
// A future `EvalResult.stdout` field on `eval_in_session` similarly needs a
// `with-out-str` facility (an `*out*` dynvar + a Rust capture primitive),
// which does not exist today and is out of scope for step 1.
// ============================================================================

/// Where a global definition was loaded from, lifted into a Rust struct from
/// the `[file line col]` shape `(source-location 'NAME)` returns (ADR-031).
/// `line` and `col` are 1-based, matching the runtime's `Pos`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// One advisory finding from the type checker. `pos` is `None` when the
/// message lacks position information (the checker doesn't yet thread spans
/// through macroexpansion — see `docs/types.md` and ADR-024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub pos: Option<Pos>,
    pub message: String,
}

/// The structured result of [`eval_in_session`]. Exactly one of `value` /
/// `error` is `Some`; `diagnostics` is independent — the advisory checker
/// runs over the source even when the eval succeeds, so the agent sees
/// warnings about code that happens to work.
///
/// Note (step 1 deferral): a `stdout` field belongs here but needs Brood-side
/// output capture (`*out*` + `with-out-str`) that does not exist yet — see
/// the module-level deferral note.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvalResult {
    pub value: Option<String>,
    pub error: Option<String>,
    pub diagnostics: Vec<Diag>,
}

/// Where the global `name` was defined, by lifting `(source-location 'NAME)`
/// (ADR-031). `None` when the name is unbound, has no recorded site (a Rust
/// builtin — it has no Brood source), or `(source-location)` itself errored.
/// Prelude globals *do* resolve, to a materialized copy of the prelude (so the
/// standard library is navigable); user/project defs resolve to their files.
///
/// `name` must be a single CST symbol token (see [`is_delimiter`] —
/// completions, hovers, and goto-def already enforce this on their inputs).
///
/// [`is_delimiter`]: crate::syntax::atom::is_delimiter
pub fn source_location(interp: &mut Interp, name: &str) -> Option<SourceLoc> {
    let cp = interp.heap.checkpoint();
    let out = match interp.eval_str(&format!("(source-location '{name})")) {
        Ok(v) => parse_source_location(&interp.heap, v),
        Err(_) => None,
    };
    interp.heap.reset_local_to(cp);
    out
}

/// Every `.blsp` file the bootstrapped project owns — its sources plus its
/// tests — by asking the Brood side `(project--all-files *project-root*)` (the
/// same set `check-project` walks). Empty when no project has been set up (e.g.
/// a bare buffer outside a project, where `*project-root*` is unbound). Feeds
/// the cross-file reference / rename sweep (ADR-031 §Cross-file): under the flat
/// module model these files are the whole search space for a global.
pub fn project_files(interp: &mut Interp) -> Vec<String> {
    let cp = interp.heap.checkpoint();
    let out = match interp.eval_str("(project--all-files *project-root*)") {
        Ok(v) => interp
            .heap
            .list_to_vec(v)
            .map(|items| {
                items
                    .into_iter()
                    .filter_map(|x| match x {
                        Value::Str(id) => Some(interp.heap.string(id).to_string()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    interp.heap.reset_local_to(cp);
    out
}

/// The on-disk file a `require`able feature resolves to, found the same way
/// `require` itself does: `require--find` over the live `*load-path*` (which
/// `bootstrap_project` extends with the project's source dirs). Powers
/// goto-definition on the module name in `(require 'foo)`. `None` for a baked-in
/// std module (it has no file — it's `%builtin-module` source) or a feature not
/// on the path. `feature` is a CST symbol token; it's escaped before embedding.
pub fn module_file(interp: &mut Interp, feature: &str) -> Option<String> {
    let cp = interp.heap.checkpoint();
    let expr = format!(
        "(require--find \"{}.blsp\" *load-path*)",
        escape_brood_string(feature)
    );
    let out = match interp.eval_str(&expr) {
        Ok(Value::Str(id)) => Some(interp.heap.string(id).to_string()),
        _ => None,
    };
    interp.heap.reset_local_to(cp);
    out
}

/// Lift the `[file line col]` vector `source-location` returns into a
/// [`SourceLoc`]. `Nil` (no recorded site) and any other shape become `None`.
fn parse_source_location(heap: &Heap, v: Value) -> Option<SourceLoc> {
    if matches!(v, Value::Nil) {
        return None;
    }
    let items = heap.seq_items(v).ok()?;
    if items.len() != 3 {
        return None;
    }
    let file = match items[0] {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return None,
    };
    let line = match items[1] {
        Value::Int(n) if n > 0 => n as u32,
        _ => return None,
    };
    let col = match items[2] {
        Value::Int(n) if n > 0 => n as u32,
        _ => return None,
    };
    Some(SourceLoc { file, line, col })
}

/// Expand the macros in one source form and return the result pretty-printed.
/// With `recursive == false`, runs `macroexpand-1` (a single step — useful for
/// teaching the agent what one layer of a macro turned into); with
/// `recursive == true`, runs `macroexpand` (fully expand all macros, the form
/// the compile pass actually evaluates — useful when debugging quasiquote or
/// nested-macro emission).
///
/// `src` must read as **exactly one** top-level form. Multi-form input is
/// **rejected** (rather than silently expanding only the first) — silently
/// dropping forms hides agent misuse where they meant to chain expansions;
/// returning an error makes the contract obvious. Wrap explicitly in
/// `(do …)` to expand a sequence as one form.
///
/// Parses the source ourselves rather than going through
/// `eval_str("(macroexpand-1 'SRC)")` — the latter would let `src` break out
/// of the surrounding expression on any unbalanced paren or stray quote.
pub fn macroexpand_to_string(
    interp: &mut Interp,
    src: &str,
    recursive: bool,
) -> Result<String, String> {
    let cp = interp.heap.checkpoint();
    let result = (|| -> Result<String, String> {
        let forms =
            crate::syntax::reader::read_all(&mut interp.heap, src).map_err(|e| e.to_string())?;
        if forms.len() > 1 {
            return Err(format!(
                "expected exactly one form, got {} (wrap multiple forms in `(do …)`)",
                forms.len()
            ));
        }
        let form = forms.into_iter().next().ok_or("no form to expand")?;
        let expanded = if recursive {
            crate::eval::macros::macroexpand(&mut interp.heap, form, interp.root)
        } else {
            crate::eval::macros::macroexpand_1(&mut interp.heap, form, interp.root).map(|(v, _)| v)
        }
        .map_err(|e| e.to_string())?;
        // Build the printed form *before* the LOCAL reset below — once the
        // heap is rolled back, `expanded`'s pairs are gone.
        Ok(interp.print(expanded))
    })();
    interp.heap.reset_local_to(cp);
    result
}

/// Reformat a Brood source string by routing through `std/format.blsp`'s
/// `(format-source SRC)`. Idempotent (`format(format(x)) == format(x)`).
/// `src` is interpolated into a Brood string literal, so it can contain
/// arbitrary whitespace including newlines — only `\` and `"` need escaping
/// (the reader's string rule, [`read_string`]).
///
/// `Err` covers both a parse error in `src` (which `format-source` surfaces
/// through its CST) and a missing or malformed `std/format` module.
///
/// [`read_string`]: crate::syntax::reader
pub fn format_source(interp: &mut Interp, src: &str) -> Result<String, String> {
    let cp = interp.heap.checkpoint();
    let escaped = escape_brood_string(src);
    let code = format!("(require 'format) (format-source \"{escaped}\")");
    let result = match interp.eval_str(&code) {
        Ok(Value::Str(id)) => Ok(interp.heap.string(id).to_string()),
        Ok(_) => Err("format-source did not return a string".to_string()),
        Err(e) => Err(e.to_string()),
    };
    interp.heap.reset_local_to(cp);
    result
}

/// Evaluate `src` against the session's `Interp` and return the printed
/// value (on success), the formatted error (on failure), and any advisory
/// type-checker findings. The session's RUNTIME state (globals defined via
/// `def`, spawned processes) accumulates across calls — that is the point of
/// `nest mcp`'s long-lived runtime (ADR-013 hot reload).
///
/// Diagnostics come from a separate `read_all_positioned` + `check_file` pass
/// over the same source (same path the LSP takes at
/// `crates/lsp/src/main.rs:398-415`). If the source has a parse error, the
/// checker can't run and diagnostics are empty — the error captures it.
///
/// (Step 1 deferral: no `stdout` capture yet — see the module deferral note.)
pub fn eval_in_session(interp: &mut Interp, src: &str) -> EvalResult {
    // Collect diagnostics first, against a *separate* checkpoint that we
    // close before the eval — the checker's transient parse allocations
    // shouldn't pile on top of (or get tangled with) the eval's result.
    let diagnostics = collect_diagnostics(interp, src);

    // Eval bracketed by a checkpoint + reset, satisfying the module-level
    // **LOCAL-clean** contract. The trick that lets us reset safely is that
    // we render the result to an owned `String` *before* the reset — the
    // String holds no LOCAL handle, so it survives the wipe. `def`s the
    // source contains were promoted into RUNTIME by the evaluator
    // (ADR-013), so the *session state* the next call sees is unaffected;
    // only the discarded intermediate value's allocations go. Pre-fix this
    // bracket was skipped (see git blame for the prior rationale), and a
    // long agent session steadily accumulated a per-call LOCAL baseline.
    let cp = interp.heap.checkpoint();
    let (value, error) = match interp.eval_str(src) {
        Ok(v) => (Some(interp.print(v)), None),
        Err(e) => (None, Some(e.to_string())),
    };
    interp.heap.reset_local_to(cp);

    EvalResult {
        value,
        error,
        diagnostics,
    }
}

/// Run the advisory checker over `src` and return one [`Diag`] per finding.
/// Allocations from the parse + check are reclaimed before return so this
/// doesn't pile onto whatever the caller does next.
fn collect_diagnostics(interp: &mut Interp, src: &str) -> Vec<Diag> {
    use crate::syntax::reader;
    use crate::types::check::check_file;

    let cp = interp.heap.checkpoint();
    let diagnostics = match reader::read_all_positioned(&mut interp.heap, src) {
        Ok(positioned) => {
            let forms: Vec<Value> = positioned.into_iter().map(|(f, _)| f).collect();
            check_file(&mut interp.heap, &forms)
                .into_iter()
                .map(|(pos, message)| Diag { pos, message })
                .collect()
        }
        Err(_) => Vec::new(),
    };
    interp.heap.reset_local_to(cp);
    diagnostics
}

/// Escape `\` and `"` so `s` can be interpolated inside a Brood string
/// literal. Newlines and other control characters pass through verbatim —
/// the reader's `read_string` only specials those two chars (it accepts raw
/// newlines inside a literal, see `crates/lisp/src/syntax/reader.rs:280`).
///
/// Shared with `nest` / `brood-lsp` (`brood::introspect::escape_brood_string`):
/// the one place this rule lives, since drift between sites is what would
/// produce a "looks correct in source, breaks in some other escape sequence"
/// bug.
pub fn escape_brood_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build a Brood call form `(fn-name "a1" "a2" …)` with each argument embedded
/// as a properly-escaped string literal. The one place the "interpolate user
/// strings into a Brood call" pattern lives — callers in `nest` / `brood-lsp`
/// were each hand-writing `format!("(… \"{}\")", escape_brood_string(x))`,
/// which is easy to get subtly wrong (a forgotten escape, a missing space).
/// All arguments are strings, which is what every current call site needs
/// (paths, names, source); a literal/number arg would be a separate helper.
pub fn call_form(fn_name: &str, string_args: &[&str]) -> String {
    let mut s = String::from("(");
    s.push_str(fn_name);
    for a in string_args {
        s.push_str(" \"");
        s.push_str(&escape_brood_string(a));
        s.push('"');
    }
    s.push(')');
    s
}

/// Bootstrap a project image for **tooling** — the shared entry the `brood-lsp`
/// server and `nest mcp` both use so they can't drift on what a tooling image
/// contains. Routes to the Brood `(setup-tooling-image root)` in
/// `std/project.blsp`: it puts the project's sources on `*load-path*`, loads
/// them, and requires the `test` + `format` frameworks (so cross-module names
/// and framework macros resolve in the advisory checker). `root` is the already
/// resolved project-root directory.
///
/// Best-effort: an `Err` is returned (the caller logs and continues with at
/// least the prelude), never panics. The policy lives in Brood; this is the
/// thin typed seam Rust callers reach it through.
pub fn load_tooling_image(interp: &mut Interp, root: &str) -> Result<(), String> {
    let code = format!("(require 'project) {}", call_form("setup-tooling-image", &[root]));
    interp.eval_str(&code).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_names_includes_prelude_and_builtins() {
        let mut interp = Interp::new();
        let names = global_names(&mut interp);
        assert!(names.contains(&"map".to_string()), "prelude `map` missing");
        assert!(names.contains(&"+".to_string()), "builtin `+` missing");
        // `global-names` sorts, so the pool is in deterministic order.
        assert!(names.windows(2).all(|w| w[0] <= w[1]), "not sorted");
    }

    #[test]
    fn signature_of_a_prelude_fn_renders_its_params() {
        let mut interp = Interp::new();
        // `map` takes a function and a list; whatever its param names, the
        // signature must start `(map ` and list at least one parameter.
        let (sig, _doc) = signature(&mut interp, "map");
        let sig = sig.expect("map should have a signature");
        assert!(sig.starts_with("(map "), "got {sig:?}");
    }

    #[test]
    fn signature_of_an_unbound_name_is_empty() {
        let mut interp = Interp::new();
        assert_eq!(signature(&mut interp, "no-such-global-xyzzy"), (None, None));
    }

    #[test]
    fn repeated_queries_stay_correct_after_local_reset() {
        // Each query resets the LOCAL heap to reclaim its result (so a long server
        // session doesn't leak a list per keystroke). This must not clobber
        // interned/prelude state: results stay identical across many calls.
        let mut interp = Interp::new();
        let first = global_names(&mut interp);
        let first_sig = signature(&mut interp, "map");
        for _ in 0..1000 {
            assert_eq!(global_names(&mut interp), first);
            assert_eq!(signature(&mut interp, "map"), first_sig);
        }
    }

    #[test]
    fn call_form_escapes_and_spaces_arguments() {
        assert_eq!(call_form("f", &[]), "(f)");
        assert_eq!(call_form("f", &["a", "b"]), "(f \"a\" \"b\")");
        // A backslash and a quote in the argument are escaped per the reader's
        // string rule — the produced form must read back as the literal path.
        assert_eq!(
            call_form("setup-tooling-image", &["/a b/\"x\"\\y"]),
            "(setup-tooling-image \"/a b/\\\"x\\\"\\\\y\")"
        );
    }

    #[test]
    fn load_tooling_image_is_best_effort_outside_a_project() {
        // No project at this path → `setup-tooling-image` runs against an empty
        // load set; it must not panic, and the Interp stays usable afterwards.
        let mut interp = Interp::new();
        let _ = load_tooling_image(&mut interp, "/nonexistent/path/xyzzy");
        assert!(interp.eval_str("(+ 1 2)").is_ok(), "interp still usable");
    }

    // ---- step 1b — wider tooling surface ------------------------------------

    #[test]
    fn source_location_resolves_prelude_fns_but_not_builtins_or_unbound() {
        // The prelude is now loaded positioned, with `current-file` set to a
        // materialized cache copy, so a prelude `defn` like `map` reports a site
        // there — this powers `M-.` into the standard library. (Even `+` is a
        // prelude `defn` over the `%add` primitive, so it resolves too.) A Rust
        // *primitive* like `cons` has no Brood source, and an unknown name has no
        // global at all, so both still yield `None` (MCP `lookup` relies on that).
        let mut interp = Interp::new();
        let map = source_location(&mut interp, "map").expect("prelude fn has a site");
        assert!(
            map.file.ends_with("prelude.blsp"),
            "should point at the prelude copy, got {map:?}"
        );
        assert!(map.line >= 1 && map.col >= 1, "{map:?}");
        assert_eq!(source_location(&mut interp, "cons"), None);
        assert_eq!(source_location(&mut interp, "no-such-name-xyzzy"), None);
    }

    #[test]
    fn source_location_records_a_loaded_files_definitions() {
        // To populate the def-site table we need `current-file` set + the
        // positioned reader path (`eval_source`). That's exactly the file
        // loader's combination — `(current-file)` is read-only from Brood, so
        // set it from Rust directly. A path is just a string; no real file
        // has to exist for the recorded site to be observable.
        let mut interp = Interp::new();
        let prev = interp
            .heap
            .set_current_file(Some("tests/dummy.blsp".into()));
        interp.eval_source("(defn my-fn (x) (* x x))").unwrap();
        interp.heap.set_current_file(prev);

        let loc = source_location(&mut interp, "my-fn").expect("recorded");
        assert_eq!(loc.file, "tests/dummy.blsp");
        // 1-based; both must be positive and within the source's range.
        assert!(loc.line >= 1 && loc.col >= 1, "{loc:?}");
    }

    #[test]
    fn parse_source_location_lifts_a_vector_and_rejects_other_shapes() {
        // The lifter is the only piece of `source_location` that runs without a
        // recorded site, so it's worth a direct test. A 3-element `[Str Int Int]`
        // vector becomes a `SourceLoc`; anything else (wrong arity, wrong types,
        // `nil`) becomes `None`.
        let mut heap = Heap::new();
        let file = heap.alloc_string("foo.blsp");
        let ok = heap.alloc_vector(vec![file, Value::Int(10), Value::Int(3)]);
        assert_eq!(
            parse_source_location(&heap, ok),
            Some(SourceLoc {
                file: "foo.blsp".into(),
                line: 10,
                col: 3
            })
        );
        assert_eq!(parse_source_location(&heap, Value::Nil), None);
        let too_short = heap.alloc_vector(vec![Value::Int(1)]);
        assert_eq!(parse_source_location(&heap, too_short), None);
    }

    #[test]
    fn macroexpand_to_string_steps_a_when() {
        // `(when c e)` expands to `(if c (do e) nil)` in one step. Asserting on
        // the *spelling* keeps the test honest about what the agent will see —
        // a substring check that confirms the conditional branch was generated.
        let mut interp = Interp::new();
        let one_step = macroexpand_to_string(&mut interp, "(when x 1)", false).unwrap();
        assert!(one_step.contains("if"), "got {one_step:?}");
        assert!(one_step.contains("x"), "got {one_step:?}");
    }

    #[test]
    fn macroexpand_to_string_recursively_runs_to_a_fixed_point() {
        // `macroexpand` (recursive) keeps going until no more macro head is at
        // the top — so an expression that was *not* a macro head to begin with
        // round-trips unchanged. This pins the contract: recursive expansion
        // never "evaluates" anything, it only rewrites.
        let mut interp = Interp::new();
        assert_eq!(
            macroexpand_to_string(&mut interp, "(+ 1 2)", true).unwrap(),
            "(+ 1 2)"
        );
    }

    #[test]
    fn macroexpand_to_string_surfaces_parse_errors() {
        let mut interp = Interp::new();
        let err = macroexpand_to_string(&mut interp, "(unclosed", false).unwrap_err();
        assert!(!err.is_empty(), "expected a non-empty error message");
    }

    #[test]
    fn macroexpand_to_string_rejects_multi_form_input() {
        // Silently expanding only the first form hides agent misuse where they
        // meant to chain expansions. Make the contract visible: error and
        // point at the `(do …)` wrap.
        let mut interp = Interp::new();
        let err = macroexpand_to_string(&mut interp, "(when x 1) (when y 2)", false).unwrap_err();
        assert!(err.contains("exactly one form"), "{err:?}");
        assert!(err.contains("do"), "{err:?}");
    }

    #[test]
    fn format_source_reformats_a_messy_form() {
        // The formatter normalises inter-token spacing and trims trailing
        // whitespace. We don't pin the exact output (that's `std/format.blsp`'s
        // contract, not ours) — only that *something* changed and the result is
        // a string. Idempotence is checked separately below.
        let mut interp = Interp::new();
        let formatted = format_source(&mut interp, "(  +  1   2  )\n\n\n").unwrap();
        assert!(!formatted.is_empty());
        // Idempotent: formatting the formatted source again is a fixed point.
        let again = format_source(&mut interp, &formatted).unwrap();
        assert_eq!(formatted, again);
    }

    #[test]
    fn format_source_passes_through_escapes_and_newlines() {
        // `src` is interpolated into a Brood string literal — only `\` and `"`
        // get escaped, so embedded newlines and tabs must round-trip without
        // breaking the outer expression.
        let mut interp = Interp::new();
        let messy = "(def s \"a\\nb\")\n  (def t \"c\\\\d\")";
        let formatted = format_source(&mut interp, messy).unwrap();
        assert!(formatted.contains("\"a\\nb\""), "got {formatted:?}");
        assert!(formatted.contains("\"c\\\\d\""), "got {formatted:?}");
    }

    #[test]
    fn eval_in_session_returns_the_printed_value() {
        let mut interp = Interp::new();
        let r = eval_in_session(&mut interp, "(+ 1 2)");
        assert_eq!(r.value.as_deref(), Some("3"));
        assert_eq!(r.error, None);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn eval_in_session_captures_the_error() {
        let mut interp = Interp::new();
        let r = eval_in_session(&mut interp, "(no-such-fn 1)");
        assert_eq!(r.value, None);
        assert!(r.error.is_some(), "expected an error");
    }

    #[test]
    fn eval_in_session_state_persists_across_calls() {
        // The MCP session promise: a `def` in one call is visible to the next.
        // This is the hot-reload behaviour (ADR-013) — `def` mutates the
        // RUNTIME region, which survives our per-call LOCAL housekeeping.
        let mut interp = Interp::new();
        let r1 = eval_in_session(&mut interp, "(def x 42)");
        assert_eq!(r1.error, None);
        let r2 = eval_in_session(&mut interp, "(* x 2)");
        assert_eq!(r2.value.as_deref(), Some("84"));
        assert_eq!(r2.error, None);
    }

    #[test]
    fn eval_in_session_reports_advisory_diagnostics() {
        // The checker spots `+`'s arg type. We don't pin the exact message —
        // `docs/types.md` lets it evolve — only that *some* diagnostic landed
        // when the source contains a provable type misuse.
        let mut interp = Interp::new();
        let r = eval_in_session(&mut interp, "(+ 1 \"oops\")");
        assert!(!r.diagnostics.is_empty(), "expected a diagnostic");
    }
}
