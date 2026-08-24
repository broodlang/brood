#!/usr/bin/env bash
# stale-names.sh — after a rename wave, find every surviving reference to an old name,
# INCLUDING the ones no checker can see.
#
# The question this answers: "I renamed `getenv` to `os/getenv` across .blsp files — what
# did I miss?" `nest check` cannot answer it, because the misses are never in the places it
# looks:
#
#   1. Brood source embedded in a RUST STRING. `nest run` shipped completely broken because
#      `crates/nest/src/main.rs` builds `"(unless (= (getenv \"BROOD_NO_CHECK\") …"` — a
#      snippet the checker never sees, in a crate the .blsp tooling never reads. Same for
#      the `.blsp` fixtures that `crates/cli/tests/distribution.rs` writes out.
#   2. QUOTED Brood that is really code. `'(spawn (apprun-beat (now)))` is data to the
#      reader and to `nest rename` (correctly — a quoted symbol is usually a registry key),
#      but it is evaluated later, so a stale name there fails at runtime in generated code.
#   3. A name used as a VALUE, not a call. `(or (get model :now-fn) now)` has no `(now `
#      to match, so a call-position rename walks straight past it.
#   4. Error-message and docstring PROSE that tells a user to call something. Harmless to
#      the build, actively misleading to the reader — `(process-flag :max-heap n)` in a
#      hint naming a function that no longer exists.
#
# A general "check every embedded snippet" lint was prototyped and rejected: it produces
# ~160 candidate warnings that are overwhelmingly docstring prose, deliberately-unbound test
# fixtures (`no-such-fn`), and record/ability names defined inside the snippet. The targeted
# form — grep for the specific names you just moved — found all eleven real misses in
# seconds, with no false-positive budget to maintain.
#
# Usage:
#   scripts/stale-names.sh RENAMES [DIR …]
#
#   RENAMES  a file of `old new` pairs, one per line (`#` comments and blanks ignored).
#            `new` is only used for the report; matching is on `old`.
#   DIR …    roots to search (default: the current directory). Pass sibling repos to sweep
#            the whole ecosystem: scripts/stale-names.sh /tmp/renames.txt . ../hatch ../hive
#
# Exit status is 1 if anything matched, so it works as a CI gate mid-rename.
#
# Matching uses the Brood identifier boundary (letters, digits, and `-_?!*<>=+.%&~^` are all
# identifier characters, `/` is not), so `getenv` does NOT match inside `os/getenv`, and
# `now` does NOT match inside `now-ms`.

set -uo pipefail

RENAMES=${1:-}
if [[ -z $RENAMES || ! -f $RENAMES ]]; then
  echo "usage: $0 RENAMES [DIR …]   (RENAMES: a file of 'old new' lines)" >&2
  exit 2
fi
shift
DIRS=("$@")
[[ ${#DIRS[@]} -eq 0 ]] && DIRS=(.)

# Brood identifier characters, for the word boundary. `/` is deliberately excluded so a
# bare `getenv` does not match the already-migrated `os/getenv`.
# NOTE: `-` is deliberately NOT here — it is appended LAST when the class is built.
# Inside a bracket expression a `-` between two characters is a RANGE, so writing the
# class as [^…^-/] silently means "^ through /" (a reversed range) and the whole check
# quietly matches nothing. This script's first version had exactly that bug and reported
# a clean tree over a deliberately reintroduced stale name.
IDENT='A-Za-z0-9_?!*<>=+.%&~^'

total=0
while read -r old new _rest; do
  [[ -z ${old:-} || ${old:0:1} == "#" ]] && continue
  # Escape regex metacharacters that are legal in Brood names (?, *, +, ., ^).
  esc=$(printf '%s' "$old" | sed -E 's/[?*+.^$]/\\&/g')
  hits=$(grep -rInE "(^|[^${IDENT}/-])${esc}([^${IDENT}/-]|$)" "${DIRS[@]}" \
           --include='*.blsp' --include='*.rs' --include='*.md' \
           --exclude-dir=_deps --exclude-dir=.git --exclude-dir=target \
           2>/dev/null)
  if [[ -n $hits ]]; then
    n=$(printf '%s\n' "$hits" | wc -l)
    total=$((total + n))
    printf '\n=== %s -> %s  (%d)\n' "$old" "${new:-?}" "$n"
    printf '%s\n' "$hits" | sed 's/^/  /'
  fi
done < "$RENAMES"

if [[ $total -eq 0 ]]; then
  echo "no stale references to any renamed name"
  exit 0
fi
printf '\n%d reference(s) to a renamed name remain.\n' "$total"
echo "Triage each: executable code and user-facing prose must move; a deliberate mention"
echo "of the historical name (a CHANGELOG entry, an ADR) should stay."
exit 1
