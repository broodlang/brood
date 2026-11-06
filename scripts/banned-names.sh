#!/usr/bin/env bash
# banned-names.sh — Brood is described in its own terms. No file a reader meets frames it by
# another runtime: not as a clone, a port, a parity target or a benchmark baseline. A scrub
# (2026-08-06) removed ~900 such mentions; this keeps them from drifting back in one at a time.
#
# Scope: every tracked file except history, which records what was thought at the time and
# stays as written (the devlog, the decision records, the archive, the changelog, the research
# notes that cite the type system's papers) and generated files.
#
# The rule, per name:
#   - four are banned outright (the first four alternatives in `pattern`);
#   - the fifth is also a language the editor edits through tree-sitter, so it may appear as
#     an identifier anywhere (`:lang`, `tree-sitter-lang`, `"lang"`, `lang/…`, in backticks)
#     and as prose only in the editor's language-support files (`prose_paths`).
#   - `allowed_lines`: the few fixed strings that are neither, each with its reason.
#
# Usage: scripts/banned-names.sh   (exit 1, listing each hit, when one is found)
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1

pattern='\b(erlang|beam|otp|ets|elixir)\b'
banned='\b(erlang|beam|otp|ets)\b'
identifier='[-:/"`]elixir|elixir[-/"`]'

history=(
    ':!docs/devlog.md' ':!docs/devlog/*' ':!docs/decisions.md' ':!docs/decisions/*'
    ':!docs/archive/*' ':!docs/research/*' ':!CHANGELOG.md' ':!Cargo.lock'
    ':!scripts/banned-names.sh'
)

prose_paths=(
    std/editor/
    crates/lisp/src/host/treesit
    tests/tree_sitter_test.blsp
    tests/treesit_module_test.blsp
)

allowed_lines=(
    # docs/types.md cites the paper the set-theoretic checker is built on, by its title.
    'The Design Principles of the Elixir Type'
)

prose_allowed() { # $1 = path
    local prefix
    for prefix in "${prose_paths[@]}"; do
        case "$1" in "$prefix"*) return 0 ;; esac
    done
    return 1
}

line_allowed() { # $1 = line text
    local text
    for text in "${allowed_lines[@]}"; do
        case "$1" in *"$text"*) return 0 ;; esac
    done
    return 1
}

hits=""
while IFS= read -r hit; do
    path=${hit%%:*}
    rest=${hit#*:}
    text=${rest#*:}
    if printf '%s\n' "$text" | grep -qiE "$banned"; then
        hits="$hits
  $hit"
        continue
    fi
    line_allowed "$text" && continue
    prose=$(printf '%s\n' "$text" | sed -E "s/$identifier//gI")
    printf '%s\n' "$prose" | grep -qiE "$pattern" || continue
    prose_allowed "$path" && continue
    hits="$hits
  $hit"
done < <(git grep -n -I -i -E "$pattern" -- . "${history[@]}")

if [ -n "$hits" ]; then
    echo "banned-names: another runtime is named in a forward-facing file:$hits" >&2
    echo "  describe Brood in its own terms; scripts/banned-names.sh says what is allowed" >&2
    exit 1
fi
echo "banned-names: clean"
