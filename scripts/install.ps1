# Brood installer for Windows. Usage, in PowerShell:
#   irm https://brood.fly.dev/install.ps1 | iex
#
# Downloads the prebuilt brood + nest + brood-lsp from the latest GitHub release, checks the
# archive against the release's .sha256, and installs them (ADR-714). Override with env vars:
#   $env:BROOD_VERSION = "v0.51.0"      install a specific release (default: latest)
#   $env:BROOD_INSTALL_DIR = "C:\path"  install location (default: %LOCALAPPDATA%\brood\bin)
$ErrorActionPreference = "Stop"

$repo = "broodlang/brood"
$target = "x86_64-pc-windows-msvc"
$installDir = if ($env:BROOD_INSTALL_DIR) { $env:BROOD_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "brood\bin" }

function Say($message) { Write-Host "brood-install: $message" }
function Fail($message) { Write-Host "brood-install: error: $message" -ForegroundColor Red; exit 1 }

if ($env:PROCESSOR_ARCHITECTURE -ne "AMD64") {
  Fail "the Windows build is x86_64 only; this machine is $($env:PROCESSOR_ARCHITECTURE)"
}

$version = $env:BROOD_VERSION
if (-not $version) {
  Say "resolving latest release..."
  $version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
  if (-not $version) { Fail "could not determine the latest release (set BROOD_VERSION)" }
}

$name = "brood-$version-$target"
$url = "https://github.com/$repo/releases/download/$version/$name.zip"
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("brood-install-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $temporary | Out-Null
try {
  $archive = Join-Path $temporary "$name.zip"
  Say "downloading $name..."
  try { Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing } catch { Fail "download failed: $url" }

  # The check is not optional: a sidecar that will not download stops the install.
  try { $sidecar = (Invoke-WebRequest -Uri "$url.sha256" -UseBasicParsing).Content } catch { Fail "could not download $url.sha256 to check the archive against" }
  if ($sidecar -is [byte[]]) { $sidecar = [System.Text.Encoding]::UTF8.GetString($sidecar) }
  $expected = ($sidecar.Trim() -split '\s+')[0].ToLower()
  $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLower()
  if ($expected -ne $actual) { Fail "checksum mismatch (expected $expected, got $actual)" }
  Say "checksum ok"

  Expand-Archive -Path $archive -DestinationPath $temporary -Force
  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  foreach ($binary in "brood.exe", "nest.exe", "brood-lsp.exe") {
    Copy-Item -Force (Join-Path $temporary "$name\$binary") (Join-Path $installDir $binary)
  }
  Say "installed brood $version to $installDir"
} finally {
  Remove-Item -Recurse -Force $temporary -ErrorAction SilentlyContinue
}

# On PATH for new shells (the user's PATH, no administrator needed), and for this one.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not (($userPath -split ';') -contains $installDir)) {
  [Environment]::SetEnvironmentVariable("Path", (($userPath, $installDir) | Where-Object { $_ }) -join ';', "User")
  Say "added $installDir to your PATH (open a new terminal to pick it up)"
}
if (-not (($env:Path -split ';') -contains $installDir)) { $env:Path = "$env:Path;$installDir" }
& (Join-Path $installDir "brood.exe") --version
