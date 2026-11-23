#!/usr/bin/env bash
# Turn a test log into what a reader of the run sees first: one GitHub annotation per failing
# test — at its file and line, with the assertion, the actual and the expected value — and the
# same list in the job summary. Without it, finding what failed meant downloading the whole job
# log (thousands of lines) and searching it.
#
#   scripts/ci-failure-summary.sh LOG LABEL
#
# Reads three shapes: a file that did not load (`file:line:col: error: …`), the in-language
# runner's
#     tests/x_test.blsp:36: test failed: group › name
#         assert: …   actual: …   expect: …
# and a Rust test's
#     thread 'name' panicked at crates/…/file.rs:389:5:
#     message
# At most 30 of each; always exits 0, so it never hides the step's own failure.
set -uo pipefail
log="${1:?usage: ci-failure-summary.sh LOG LABEL}"
label="${2:-tests}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"

if [ ! -f "$log" ]; then
  echo "::error::$label: no log at $log — the step failed before running its tests"
  exit 0
fi

clean="$(mktemp)"
sed 's/\x1b\[[0-9;]*m//g; s/^[0-9T:.-]*Z //' "$log" > "$clean"

# An annotation's message keeps its lines when they are joined with %0A.
escape() { sed 's/%/%25/g' | awk 'BEGIN{ORS="%0A"} {print}' | sed 's/%0A$//'; }

found=0
{
  echo "### $label: what failed"
  echo
} >> "$summary"

# Brood tests: the header line, then up to three detail lines (assert / actual / expect / error).
while IFS= read -r number; do
  header="$(sed -n "${number}p" "$clean" | sed 's/^ *//')"
  location="${header%%: test failed:*}"
  file="${location%%:*}"
  rest="${location#*:}"
  line="${rest%%:*}"
  title="${header#*test failed: }"
  details="$(sed -n "$((number + 1)),$((number + 4))p" "$clean" \
    | grep -E '^\s+(assert|actual|expect|note|uncaught error|error):' | sed 's/^ *//')"
  message="$(printf '%s\n%s\n' "$title" "$details" | escape)"
  echo "::error file=$file,line=$line,title=$label: test failed::$message"
  {
    echo "- \`$file:$line\` $title"
    printf '%s\n' "$details" | sed 's/^/    /'
  } >> "$summary"
  found=$((found + 1))
done < <(grep -nE '^\s*[^ ]+\.blsp:[0-9]+(:[0-9]+)?: test failed:' "$clean" | cut -d: -f1 | head -30)

# Rust tests: the panic location and the line after it.
while IFS= read -r number; do
  header="$(sed -n "${number}p" "$clean")"
  name="$(printf '%s' "$header" | sed -n "s/.*thread '\([^']*\)'.*/\1/p")"
  where="$(printf '%s' "$header" | sed -n 's/.* panicked at \([^ ]*\):$/\1/p')"
  file="${where%%:*}"
  rest="${where#*:}"
  line="${rest%%:*}"
  detail="$(sed -n "$((number + 1)),$((number + 3))p" "$clean" | sed 's/^ *//')"
  message="$(printf '%s\n%s\n' "$name" "$detail" | escape)"
  echo "::error file=$file,line=$line,title=$label: panicked::$message"
  echo "- \`$where\` $name: $(printf '%s' "$detail" | head -1)" >> "$summary"
  found=$((found + 1))
done < <(grep -nE "thread '[^']*' (\([0-9]+\) )?panicked at [^ ]+:[0-9]+:[0-9]+:$" "$clean" \
  | grep -v "brood-suite\|_suite_passes" | cut -d: -f1 | head -30)

# A file that did not load or compile (a header error, a module that does not compile): its
# `file:line:col: error: message` line, which stops a run before any test reports.
while IFS= read -r header; do
  location="${header%%: error: *}"
  message="${header#*: error: }"
  file="${location%%:*}"
  rest="${location#*:}"
  line="${rest%%:*}"
  echo "::error file=$file,line=$line,title=$label: error::$(printf '%s\n' "$message" | escape)"
  echo "- \`$file:$line\` $message" >> "$summary"
  found=$((found + 1))
done < <(grep -oE '[^ ]+\.blsp:[0-9]+:[0-9]+: error: .*' "$clean" | sort -u | head -30)

if [ "$found" -eq 0 ]; then
  echo "::error::$label failed, but no failing test was found in the log: see the step's output"
  echo "- no failing test found in the log; see the step's output" >> "$summary"
fi
rm -f "$clean"
exit 0
