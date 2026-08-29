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
# `target/release` first only as a tiebreak (it is what CI builds, i.e. DEFAULT features);
# a candidate reporting HEAD's sha wins over both. If none exists, echo the `target/release`
# path anyway so the caller's error message names a real, buildable path.
gate_pick() {
    local base="$1" cand first="" head
    head="$(gate_head_sha)"
    for cand in "$ROOT/target/release/$base" "$ROOT/target/release-fast/$base"; do
        [ -x "$cand" ] || continue
        [ -n "$first" ] || first="$cand"
        if [ -n "$head" ] && [ "$(gate_sha_of "$cand")" = "$head" ]; then echo "$cand"; return; fi
    done
    echo "${first:-$ROOT/target/release/$base}"
}

# gate_require_fresh <path> — exit 2 unless <path> exists and its baked-in `std/`+`crates/`
# is the tree's. Prints what to do. Rule 2: this SKIPS the gate rather than colouring it.
gate_require_fresh() {
    local bin="$1" head bin_sha newer
    if [ ! -x "$bin" ]; then
        echo "the gate DID NOT RUN — no binary at $bin" >&2
        echo "  build with \`make release\` (target/release-fast) or \`cargo build --release -p cli -p nest\`" >&2
        exit 2
    fi
    head="$(gate_head_sha)"; bin_sha="$(gate_sha_of "$bin")"
    # Rule 3's exemption. `crates/*/tests/` is excluded: an integration test compiles into its
    # own test binary, never into these, so it cannot change what a gate sees.
    if [ -n "$head" ] && [ -n "$bin_sha" ] && [ "$bin_sha" != "$head" ]; then
        if git -C "$ROOT" cat-file -e "${bin_sha}^{commit}" 2>/dev/null &&
           [ -z "$(git -C "$ROOT" diff --name-only "$bin_sha" HEAD -- std crates \
                     ':(exclude)crates/*/tests/*' 2>/dev/null)" ]; then
            echo "note: $bin is from $bin_sha, but nothing it bakes in changed since — its verdict is current" >&2
        else
            echo "the gate DID NOT RUN — $bin is built from $bin_sha, HEAD is $head" >&2
            echo "  rebuild with \`make release\`, then re-run. A stale binary bakes in a stale std/." >&2
            exit 2
        fi
    fi
    # The case a sha cannot see at all: uncommitted work newer than the binary.
    if newer="$(find "$ROOT/std" "$ROOT/crates" \( -name '*.blsp' -o -name '*.rs' \) \
                  -not -path "$ROOT/crates/*/tests/*" -newer "$bin" -print -quit 2>/dev/null)";
       [ -n "$newer" ]; then
        echo "the gate DID NOT RUN — $newer is newer than $bin (uncommitted work)" >&2
        echo "  rebuild with \`make release\`, then re-run." >&2
        exit 2
    fi
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
