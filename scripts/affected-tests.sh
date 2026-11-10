#!/bin/sh
# Print the tests that name what this change touches, one per line, without running them.
#
# The change set is the commits ahead of the upstream AND the working tree (staged, unstaged
# and untracked), the same set the pre-push hook gates. Output lines:
#
#   tests/foo_test.blsp          an in-language test file (`nest test` takes these)
#   cargo:-p CRATE --test NAME   a changed Rust integration test (`cargo nextest run` args)
#
# The selection rules (the pre-push hook sources this file, so they are spelled once):
#   - `std/**/foo-bar.blsp` → `tests/foo_bar_test.blsp` when it exists
#   - every changed `tests/*_test.blsp`
#   - any std/ change → the surface audit and the executed doc examples
#   - a std/prelude/ change → the bare-name ledger and the macro-capture gate
#   - a crates/lisp/src/types/ change → the checker's expectation files
#
# `--files` reads the change set from stdin instead of git (one path per line).
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1

if [ "${1:-}" = "--files" ]; then
    changed=$(cat)
else
    upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || echo origin/main)
    changed=$( { git diff --name-only "$upstream..HEAD" 2>/dev/null; \
                 git diff --name-only HEAD 2>/dev/null; \
                 git diff --name-only --cached 2>/dev/null; \
                 git ls-files --others --exclude-standard 2>/dev/null; } | sort -u )
fi

selected=""
add() { [ -f "$1" ] && case " $selected " in *" $1 "*) ;; *) selected="$selected $1" ;; esac; }

std_changed=0
for path in $(printf '%s\n' "$changed" | grep -E '\.blsp$'); do
    case "$path" in
        std/*)
            std_changed=1
            add "tests/$(basename "$path" .blsp | tr '-' '_')_test.blsp" ;;
        tests/*_test.blsp)
            add "$path" ;;
    esac
done
if [ "$std_changed" -eq 1 ]; then
    add tests/audit_test.blsp
    add tests/doc_examples_test.blsp
fi
if printf '%s\n' "$changed" | grep -q '^std/prelude/'; then
    add tests/bare_names_test.blsp
    add tests/prelude_capture_test.blsp
fi
if printf '%s\n' "$changed" | grep -q '^crates/lisp/src/types/'; then
    add tests/sig_adoption_test.blsp
    add tests/checker_pass_agreement_test.blsp
fi
for test_file in $selected; do echo "$test_file"; done

# Changed Rust integration tests, as the nextest arguments that run exactly that binary.
printf '%s\n' "$changed" | grep -E '^crates/[^/]+/tests/[^/]+\.rs$' | while read -r path; do
    [ -f "$path" ] || continue
    crate_dir=$(echo "$path" | cut -d/ -f2)
    crate=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "crates/$crate_dir/Cargo.toml" | head -1)
    echo "cargo:-p $crate --test $(basename "$path" .rs)"
done
