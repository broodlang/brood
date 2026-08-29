# shellcheck shell=bash
# Which binary does a corpus gate run? — sourced by `check-examples.sh`, `check-stress.sh`
# and `check-corpora.sh`. Not executable on its own; set `ROOT` first.
#
# WHY THIS EXISTS. `make release` builds RELEASE_DIR = `target/release-fast`; only a plain
# `cargo build --release` writes `target/release`. All three gates defaulted to
# `target/release` alone while their error told you to "run `make release-brood` first" —
# a target that writes the OTHER path. So locally the gate could not run, and the command it
# named could not fix that. This is KI-76's wrong-artifact class, which cost 9 commits of
# drift and two phantom `unbound symbol` failures; `green.sh` carries the fix inline and
# these three did not.
#
# Three rules, all of them KI-76's:
#
#   1. **Prefer the candidate that reports HEAD's sha.** `std/` is `include_str!`'d, so a
#      binary from another commit bakes in another `std/`: its verdict is about that tree,
#      not this one. Existence is the fallback, never the preference.
#   2. **A stale binary is a FAILURE that skips the gate, not a note beside a verdict.** A
#      gate that runs anyway and prints "clean" has answered a question nobody asked.
#   3. **But do not cry wolf.** A binary from a different commit is still current when
#      nothing it bakes in changed since (`git diff <sha> HEAD -- std crates` empty) —
#      without that exemption every docs-only commit refuses the gate, and the reflex
#      becomes to stop believing it.
#
# It also answers a second question the path alone cannot: **which modules does this binary
# carry?** See `gate_absent_module` below.

: "${ROOT:?gate-binary.sh: set ROOT to the repo root before sourcing}"

gate_head_sha() { git -C "$ROOT" rev-parse --short HEAD 2>/dev/null; }

# The sha `<binary> --version` reports — `brood 0.15.0 (adc5c775)`.
gate_sha_of() { "$1" --version 2>/dev/null | sed -n 's/.*(\([0-9a-f]\{7,\}\)).*/\1/p'; }

# gate_pick <basename> — echo the best candidate path.
# A candidate reporting HEAD's sha wins outright; failing that the NEWEST one wins (a
# binary built from a dirty tree records its parent's sha, so a fixed order can hand you a
# genuinely old binary over a current one). If none exists, echo the `target/release` path
# anyway so the caller's error message names a real, buildable path.
gate_pick() {
    local base="$1" cand first="" head
    head="$(gate_head_sha)"
    for cand in "$ROOT/target/release/$base" "$ROOT/target/release-fast/$base"; do
        [ -x "$cand" ] || continue
        if [ -n "$head" ] && [ "$(gate_sha_of "$cand")" = "$head" ]; then echo "$cand"; return; fi
        # No sha match: keep the NEWEST candidate, not the first that exists. A binary built
        # from a dirty tree records its PARENT's sha, so a fixed order can hand you a
        # genuinely old binary over a current one.
        { [ -z "$first" ] || [ "$cand" -nt "$first" ]; } && first="$cand"
    done
    echo "${first:-$ROOT/target/release/$base}"
}

# gate_require_fresh <path> — exit 2 unless <path> exists and its baked-in `std/`+`crates/`
# is the tree's. Prints what to do. Rule 2: this SKIPS the gate rather than colouring it.
#
# **mtime decides, not the sha**, and the order matters. A binary whose mtime is newer than
# every `std/`+`crates/` source baked exactly what is on disk now — which is the property the
# gate actually needs, and it is stronger than any sha comparison. Checking the sha first
# refuses correct work in a case that happens constantly here: build from a dirty tree (the
# binary records the sha of the HEAD it was built AT), then commit, and the sha now points at
# the parent while the contents are current. It refused this very gate on 2026-08-29. Rule 3
# again — a gate that cries wolf stops being read, and this one had the wolf built in.
#
# The sha is kept as the RESCUE for the one case mtime cannot judge: a `git checkout` (or a
# fresh clone) rewrites mtimes without changing content, so sources look newer than a binary
# that is in fact current. Then, and only then, ask git whether anything it bakes in moved.
gate_require_fresh() {
    local bin="$1" head bin_sha newer
    if [ ! -x "$bin" ]; then
        echo "the gate DID NOT RUN — no binary at $bin" >&2
        echo "  build with \`make release\` (target/release-fast) or \`cargo build --release -p cli -p nest\`" >&2
        exit 2
    fi
    # `crates/*/tests/` is excluded: an integration test compiles into its OWN test binary,
    # never into these, so it cannot change what a gate sees.
    newer="$(find "$ROOT/std" "$ROOT/crates" \( -name '*.blsp' -o -name '*.rs' \) \
               -not -path "$ROOT/crates/*/tests/*" -newer "$bin" -print -quit 2>/dev/null)"
    [ -z "$newer" ] && return 0                      # baked what is on disk — done

    head="$(gate_head_sha)"; bin_sha="$(gate_sha_of "$bin")"
    if [ -n "$head" ] && [ -n "$bin_sha" ] && [ "$bin_sha" != "$head" ] &&
       git -C "$ROOT" cat-file -e "${bin_sha}^{commit}" 2>/dev/null &&
       [ -z "$(git -C "$ROOT" diff --name-only "$bin_sha" HEAD -- std crates \
                 ':(exclude)crates/*/tests/*' 2>/dev/null)" ] &&
       [ -z "$(git -C "$ROOT" status --porcelain -- std crates 2>/dev/null)" ]; then
        echo "note: $bin is from $bin_sha and older than $newer, but nothing it bakes in changed — its verdict is current" >&2
        return 0
    fi
    echo "the gate DID NOT RUN — $newer is newer than $bin" >&2
    echo "  rebuild with \`make release\` (or \`cargo build --release -p cli -p nest\`), then re-run." >&2
    echo "  A stale binary bakes in a stale std/, so its verdict is about a different tree." >&2
    exit 2
}

# ── which modules does this binary carry? ────────────────────────────────────────────────
#
# `make release` builds `brood` with RUN_FEATURES, which is LEAN: `--no-default-features`
# compiles the DEV_MODULES (`test` `docs` `grammar` `observer` `reload` `mcp` `perf` `repl`)
# away entirely, so `examples/hot-reload/main.blsp` dies on
# `unbound symbol: reload/on-change`. That is a missing FEATURE, not rename rot — and a gate
# that reports it as an example failure sends you hunting the wrong thing (it nearly did).
#
# Ask the binary, do not guess from its path: `(builtin-modules)` is exactly this list. And
# derive the DEV set from the TREE rather than restating the Rust list here — a module the
# tree has under `std/` and the binary does not is a feature absence by construction, so
# this cannot drift when that list changes.
GATE_MODULES=""
gate_load_modules() {
    local bin="$1" probe
    probe="$(mktemp -t gate-modules-XXXXXX.blsp)" || return 1
    printf '%s\n' '(io/puts (string/join " " (map ->string (builtin-modules))))' > "$probe"
    GATE_MODULES=" $("$bin" "$probe" 2>/dev/null) "
    rm -f "$probe"
}

# gate_absent_module <ns> — true when <ns> is a std module this TREE has but this BINARY
# does not carry. Precise on purpose: a name in a module that exists NOWHERE is rename rot
# and must still fail.
gate_absent_module() {
    local ns="$1"
    case "$GATE_MODULES" in *" $ns "*) return 1 ;; esac
    [ -f "$ROOT/std/tool/$ns.blsp" ] || [ -f "$ROOT/std/$ns.blsp" ]
}

# gate_classify <output> — read a program's diagnostics and set GATE_VERDICT to
# `ok` | `skip` | `fail`, with the evidence in GATE_DETAIL.
# `skip` is reserved for the case above: EVERY unbound name belongs to a module this lean
# binary does not carry. One name that does not, and it is a failure.
GATE_VERDICT=""; GATE_DETAIL=""
gate_classify() {
    local out="$1" bad line ns absent=""
    bad="$(printf '%s\n' "$out" | grep -E 'unbound symbol|unbound error' | head -3)"
    GATE_DETAIL="$bad"
    if [ -z "$bad" ]; then GATE_VERDICT="ok"; return; fi
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        ns="$(printf '%s' "$line" | sed -nE 's/.*unbound symbol: ([a-z0-9-]+)\/.*/\1/p')"
        if [ -n "$ns" ] && gate_absent_module "$ns"; then
            case " $absent " in *" $ns "*) ;; *) absent="$absent $ns" ;; esac
            continue
        fi
        GATE_VERDICT="fail"; return
    done <<< "$bad"
    GATE_VERDICT="skip"
    GATE_DETAIL="needs$absent, absent from this lean build (\`cargo build --release -p cli\` for a full run)"
}
