# Install LetsWritePaper for the current user on Windows.
#
# Downloads LetsWritePaper-x64-setup.exe from the latest GitHub release and starts it.
# No clone is required:
#   irm https://raw.githubusercontent.com/amirrezaalasti/LetsWritePaper/main/install.ps1 | iex
# Set LWP_GITHUB to owner/LetsWritePaper to use a different repository.
#
# The downloaded program is the setup wizard. The app then opens its own wizard
# for your name, tectonic, and Claude Code.

$ErrorActionPreference = "Stop"

function Say([string]$Message) {
  Write-Host $Message
}

function Choose([string]$Prompt, [string]$Default) {
  if ([Console]::IsInputRedirected) { return $Default }
  $reply = Read-Host "$Prompt [$Default]"
  if ([string]::IsNullOrWhiteSpace($reply)) { return $Default }
  return $reply.Trim()
}

function Is-Yes([string]$Value) {
  return $Value -match '^(y|yes)$'
}

function Get-RepoFromUrl([string]$Url) {
  if ([string]::IsNullOrWhiteSpace($Url)) { return "" }
  $Url = $Url -replace '\.git$', ''
  if ($Url -match 'github\.com[:/](.+)$') { return $Matches[1] }
  return ""
}

function In-SourceTree {
  if ([string]::IsNullOrWhiteSpace($PSScriptRoot)) { return $false }
  return Test-Path (Join-Path $PSScriptRoot "app\src-tauri\tauri.conf.json")
}

function Get-Repo {
  if ($env:LWP_GITHUB) { return $env:LWP_GITHUB }
  if (In-SourceTree -and (Test-Path (Join-Path $PSScriptRoot ".git"))) {
    try {
      $url = git -C $PSScriptRoot remote get-url origin 2>$null
      $repo = Get-RepoFromUrl $url
      if ($repo) { return $repo }
    } catch {
      return "amirrezaalasti/LetsWritePaper"
    }
  }
  return "amirrezaalasti/LetsWritePaper"
}

function Get-KindScore([string]$Name) {
  $n = $Name.ToLowerInvariant()
  if ($n.EndsWith("-setup.exe")) { return 5 }
  if ($n.EndsWith(".msi")) { return 4 }
  return 0
}

function Get-ArchScore([string]$Name) {
  $n = $Name.ToLowerInvariant()
  $arm = $env:PROCESSOR_ARCHITECTURE -eq "ARM64"
  if ($arm) {
    if ($n -match 'x64|x86_64|amd64' -and $n -notmatch 'arm64|aarch64') { return 0 }
    if ($n -match 'arm64|aarch64') { return 3 }
    if ($n -match 'universal') { return 2 }
    return 2
  }
  if ($n -match 'arm64|aarch64' -and $n -notmatch 'x64|amd64|x86_64') { return 0 }
  if ($n -match 'x64|x86_64|amd64') { return 3 }
  return 2
}

function Select-Best($Items) {
  $best = $null
  $bestScore = 0
  foreach ($item in @($Items)) {
    if ([string]::IsNullOrWhiteSpace($item)) { continue }
    $kind = Get-KindScore $item
    $arch = Get-ArchScore $item
    if ($kind -eq 0 -or $arch -eq 0) { continue }
    $total = $kind * 10 + $arch
    if ($total -gt $bestScore) {
      $bestScore = $total
      $best = $item
    }
  }
  return $best
}

function Get-LocalInstallers {
  if (-not (In-SourceTree)) { return @() }
  $bundle = Join-Path $PSScriptRoot "app\src-tauri\target\release\bundle"
  if (-not (Test-Path $bundle)) { return @() }
  return @(Get-ChildItem -Path $bundle -Recurse -File | Where-Object {
      $_.Name -like "*-setup.exe" -or $_.Extension -eq ".msi"
    } | ForEach-Object { $_.FullName })
}

function Get-ReleaseUrls([string]$Repo) {
  if ([string]::IsNullOrWhiteSpace($Repo)) { return @() }
  try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{
      "User-Agent" = "LetsWritePaper-installer"
      "Accept" = "application/vnd.github+json"
    }
  } catch {
    Say "No published release for $Repo. Draft releases stay hidden until you publish them."
    return @()
  }
  return @($release.assets | ForEach-Object { $_.browser_download_url })
}

function Build-App {
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Say "Install Rust from https://rustup.rs and run this again."
    exit 1
  }
  if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    Say "Install pnpm from https://pnpm.io and run this again."
    exit 1
  }
  Say "Building the desktop app. The first build can take several minutes."
  Push-Location (Join-Path $PSScriptRoot "app")
  try {
    pnpm install
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    pnpm tauri build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  } finally {
    Pop-Location
  }
}

Say "LetsWritePaper"
Say ""
Say "This installs the desktop app for your user account."
Say "Next, the setup wizard installs LetsWritePaper and the WebView2 runtime when Windows needs it."
Say "When the app opens, its own wizard asks for your name and can install tectonic and Claude Code."
Say "Typst preview is already included."
Say ""

$chosen = $null
if (In-SourceTree) {
  $local = @(Get-LocalInstallers)
  if ($local.Count -gt 0) {
    Say "Using an installer built in this folder."
    $chosen = Select-Best $local
  }
}
if (-not $chosen) {
  $repo = Get-Repo
  Say "Downloading the latest release from $repo…"
  $chosen = Select-Best (Get-ReleaseUrls $repo)
}

if (-not $chosen -and (In-SourceTree)) {
  if (Is-Yes (Choose "No installer was found. Build one from this folder? This needs Rust, Node, and pnpm" "N")) {
    Build-App
    $chosen = Select-Best (Get-LocalInstallers)
  }
}

if (-not $chosen) {
  Say "No installer was found for amirrezaalasti/LetsWritePaper."
  Say "Publish a v* tag, or download LetsWritePaper-x64-setup.exe from the GitHub release."
  exit 1
}

Say ""
Say "Installer: $chosen"
if (-not (Is-Yes (Choose "Install now?" "Y"))) {
  Say "Nothing was installed."
  exit 0
}

if ($chosen -match '^https?://') {
  $dest = Join-Path $env:TEMP (Split-Path $chosen -Leaf)
  Say "Downloading $(Split-Path $chosen -Leaf)"
  Invoke-WebRequest -Uri $chosen -OutFile $dest -UseBasicParsing
  $chosen = $dest
}

Say ""
Say "The setup wizard will open next."
Start-Process -FilePath $chosen -Wait
Say "Open LetsWritePaper and finish its setup wizard."
