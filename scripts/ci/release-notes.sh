#!/usr/bin/env bash
# Print the body of a GitHub Release: how to install that version, then its CHANGELOG
# section. Without it a release page is a list of 26 files and nothing says which to take.
#
#   scripts/ci/release-notes.sh TAG > notes.md
#
# The release workflow sets every release's body from it; run it by hand to fix an old one:
#   gh release edit TAG --notes-file <(scripts/ci/release-notes.sh TAG)
set -euo pipefail

tag="${1:?usage: release-notes.sh TAG}"

changes="$(awk -v heading="## $tag " '
  index($0, heading) == 1 { inside = 1; next }
  inside && /^## / { exit }
  inside { print }
' CHANGELOG.md)"
[ -n "$changes" ] || { echo "release-notes: CHANGELOG.md has no '## $tag' section" >&2; exit 1; }

cat <<NOTES
## Install

Linux and macOS, into \`~/.local/bin\`:

\`\`\`sh
curl -fsSL https://brood.fly.dev/install.sh | BROOD_VERSION=$tag sh
\`\`\`

Windows, in PowerShell, into \`%LOCALAPPDATA%\\brood\\bin\`:

\`\`\`powershell
\$env:BROOD_VERSION = "$tag"; irm https://brood.fly.dev/install.ps1 | iex
\`\`\`

Leave out \`BROOD_VERSION\` to get the newest release, and run \`nest update-brood\` later to
move to a newer one. Then:

\`\`\`sh
brood                                  # a REPL
nest new hello && cd hello && nest run
\`\`\`

The installer picks the archive for your machine and checks it against its \`.sha256\`. To
install by hand, take \`brood-$tag-<target>.tar.gz\` (\`.zip\` on Windows) and put \`brood\`,
\`nest\` and \`brood-lsp\` on your \`PATH\`. The \`brood-runtime-*\` files are not for
installing: \`nest release --target\` downloads them to build an app for another platform.

## What changed
$changes
NOTES
