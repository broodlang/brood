#!/bin/sh
# embedded-paths.sh — every file the Rust crates compile in must be in the commit.
#
# `include_str!` / `include_bytes!` and `embedded_module!` (which wraps `include_str!` with a
# repository-root path) read a file at build time. A commit that deletes or never adds such a
# file builds on the author's machine — the file is still on disk — and breaks every clean
# checkout: on 2026-11-11 a commit deleted std/tool/migrate.blsp while
# crates/lisp/src/builtins/modules.rs still embedded it, and main stopped building.
#
# The check is grep-level (well under a second): each literal path named under crates/ is
# resolved (`include_*!` relative to its file, `embedded_module!` to the repository root) and
# must be tracked. With no argument the INDEX is read — what the next commit holds; with a
# tree-ish (`HEAD`), that tree. `crates/lisp/tests/embedded_paths_tracked.rs` is the same rule
# as a test.
#
# Usage: scripts/embedded-paths.sh [TREE-ISH]   (exit 1, listing each missing path)
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1

tree=${1:-}
pattern='(include_str|include_bytes)!\("[^"]+"\)|embedded_module!\("[^"]*", *"[^"]+"\)'
# `FILE:LINE:` then the macro call, one per line; `git grep -o` keeps only the call. Exit 1
# from git grep is "no match"; anything above it is an error.
if [ -n "$tree" ]; then
    tracked=$(git ls-tree -r --name-only "$tree") || exit 2
    calls=$(git grep -n -o -E "$pattern" "$tree" -- 'crates/*.rs')
else
    tracked=$(git ls-files) || exit 2
    calls=$(git grep --cached -n -o -E "$pattern" -- 'crates/*.rs')
fi
[ $? -gt 1 ] && exit 2

missing=$(printf '%s\n' "$calls" | awk -v tree="$tree" '
    # Collapse `.` and `..` segments, so a path relative to its file compares with git'"'"'s.
    function normalize(path,    parts, count, out, depth, i) {
        count = split(path, parts, "/")
        depth = 0
        for (i = 1; i <= count; i++) {
            if (parts[i] == "" || parts[i] == ".") continue
            if (parts[i] == "..") { if (depth > 0) depth--; continue }
            out[++depth] = parts[i]
        }
        path = ""
        for (i = 1; i <= depth; i++) path = path (i > 1 ? "/" : "") out[i]
        return path
    }
    NF == 0 { next }
    {
        line = $0
        # A tree-ish prefixes each hit with `TREE:`; drop it.
        if (tree != "" && index(line, tree ":") == 1) line = substr(line, length(tree) + 2)
        first = index(line, ":")
        file = substr(line, 1, first - 1)
        rest = substr(line, first + 1)
        second = index(rest, ":")
        number = substr(rest, 1, second - 1)
        call = substr(rest, second + 1)
        count = split(call, pieces, "\"")
        if (call ~ /^embedded_module!/) {
            target = normalize(pieces[4])
        } else {
            directory = file
            sub(/\/[^\/]*$/, "", directory)
            target = normalize(directory "/" pieces[2])
        }
        print target "\t" file ":" number
    }' | while IFS='	' read -r target site; do
        if ! printf '%s\n' "$tracked" | grep -qxF "$target"; then
            echo "  $site embeds $target, which is not tracked"
        fi
    done)

if [ -n "$missing" ]; then
    printf '%s\n' "$missing" >&2
    echo "embedded-paths: a crate compiles in a file the commit does not have — git add it, or drop the embed" >&2
    exit 1
fi
exit 0
