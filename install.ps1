#Requires -Version 5.1
# =============================================================================
# IronHermes Windows Installer (install verb only)
# =============================================================================
# Usage:
#   iwr -useb https://raw.githubusercontent.com/bradwilson331/iron-hermes/main/install.ps1 | iex
#   # or, after downloading:
#   .\install.ps1
#
# Environment overrides:
#   IRONHERMES_REPO     Distribution source URL (default: https://github.com/bradwilson331/iron-hermes)
#   IRONHERMES_VERSION  Version to install (default: latest)
#
# OUT OF SCOPE (deferred, D-12): update / reinstall / uninstall verbs for Windows.
# Use the Linux/macOS install.sh for the full lifecycle on those platforms.
# =============================================================================
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# --- Constants ---
$RepoOwner  = 'bradwilson331'
$RepoName   = 'iron-hermes'
$Artifact   = 'ironhermes-windows-x86_64.tar.gz'
$BinaryName = 'ironhermes.exe'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\ironhermes'

# --- Color helpers ---
function Write-Info  { param([string]$Msg) Write-Host "[INFO]  $Msg" -ForegroundColor Cyan }
function Write-Ok    { param([string]$Msg) Write-Host "[OK]    $Msg" -ForegroundColor Green }
function Write-Warn  { param([string]$Msg) Write-Host "[WARN]  $Msg" -ForegroundColor Yellow }
function Write-Err   { param([string]$Msg) Write-Error "[ERROR] $Msg" }

# --- Security: validate IRONHERMES_REPO (T-37.1-06-01) ---
# Accept only https:// or http://192.168. (Gitea LAN); reject shell/format metacharacters.
# Mirrors install.sh::validate_repo_url.
function Test-RepoUrl {
    param([string]$Url)

    # Reject shell/format metacharacters
    $BadChars = @(';', '|', '&', '$', '`', ' ', '(', ')', '<', '>')
    foreach ($ch in $BadChars) {
        if ($Url.Contains($ch)) {
            Write-Err "IRONHERMES_REPO contains invalid character '$ch': $Url"
            exit 1
        }
    }

    # Accept https:// or http://192.168. only
    if ($Url -match '^https://') {
        return  # OK
    }
    if ($Url -match '^http://192\.168\.') {
        return  # OK — Gitea LAN mirror
    }

    Write-Err "IRONHERMES_REPO must start with https:// or http://192.168. (got: $Url)"
    exit 1
}

# --- Windows binaries not yet published (codebase not yet Windows-portable) ---
# The release pipeline does not build a Windows artifact yet: the codebase still
# uses Unix-only APIs (UnixListener/UnixStream, setpgid/killpg/SIGKILL, pre_exec)
# that don't compile for x86_64-pc-windows-msvc. Tracked for a Windows-portability
# follow-on phase. Until then this installer cannot fetch '$Artifact'.
# Escape hatch: set IRONHERMES_ALLOW_WINDOWS=1 once a Windows release asset exists.
if (-not $env:IRONHERMES_ALLOW_WINDOWS) {
    Write-Warn "IronHermes does not yet publish prebuilt Windows binaries."
    Write-Warn "The codebase is being made Windows-portable in a follow-on phase."
    Write-Host ""
    Write-Host "  In the meantime, on Windows you can:" -ForegroundColor White
    Write-Host "    - Use WSL2 and run the Linux installer:" -ForegroundColor White
    Write-Host "        curl -fsSL https://raw.githubusercontent.com/$RepoOwner/$RepoName/main/install.sh | bash" -ForegroundColor White
    Write-Host "    - Or build from source: cargo install --git https://github.com/$RepoOwner/$RepoName ironhermes" -ForegroundColor White
    Write-Host ""
    exit 1
}

# --- Resolve default repo URL ---
$Repo = if ($env:IRONHERMES_REPO) { $env:IRONHERMES_REPO } else { "https://github.com/$RepoOwner/$RepoName" }
Test-RepoUrl -Url $Repo

# --- Resolve version ---
$Version = $env:IRONHERMES_VERSION
if (-not $Version -or $Version -eq 'latest') {
    Write-Info "Resolving latest release from GitHub API..."
    try {
        $ApiUrl   = "https://api.github.com/repos/$RepoOwner/$RepoName/releases/latest"
        $Release  = Invoke-RestMethod -Uri $ApiUrl -Headers @{ 'User-Agent' = 'ironhermes-installer' }
        $Version  = $Release.tag_name
        Write-Info "Latest version: $Version"
    } catch {
        Write-Err "Could not resolve latest version from GitHub API: $_"
        exit 1
    }
}

# --- Download and install ---
Write-Host ""
Write-Host "IronHermes Installer (Windows)" -ForegroundColor White
Write-Host "========================================" -ForegroundColor White
Write-Host ""

Write-Info "Installing version: $Version"
Write-Info "Install directory:  $InstallDir"

$DownloadUrl = "https://github.com/$RepoOwner/$RepoName/releases/download/$Version/$Artifact"
Write-Info "Downloading $Artifact..."

$TmpDir = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), "ironhermes-install-$([System.Guid]::NewGuid().ToString('N'))")
New-Item -ItemType Directory -Path $TmpDir | Out-Null

try {
    $TarballPath = Join-Path $TmpDir $Artifact

    # Download tarball
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $TarballPath -UseBasicParsing
    Write-Ok "Download complete"

    # Extract with tar (ships on Windows 10 1803+ via bsdtar)
    Push-Location $TmpDir
    try {
        & tar -xzf $TarballPath
    } finally {
        Pop-Location
    }

    # Locate ironhermes.exe in the extracted tree
    $ExePath = Get-ChildItem -Path $TmpDir -Filter $BinaryName -Recurse -ErrorAction SilentlyContinue |
               Select-Object -First 1 -ExpandProperty FullName
    if (-not $ExePath) {
        Write-Err "Could not find $BinaryName in the extracted tarball."
        exit 1
    }

    # Create install dir and copy binary
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir | Out-Null
    }
    Copy-Item -Path $ExePath -Destination (Join-Path $InstallDir $BinaryName) -Force
    Write-Ok "Binary installed to $InstallDir\$BinaryName"

    # --- PATH: append install dir to user PATH (idempotent, T-37.1-06-03) ---
    $CurrentPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $CurrentPath) { $CurrentPath = '' }

    # Normalise: split on ';', trim, drop empties, compare case-insensitively
    $PathEntries = $CurrentPath -split ';' | Where-Object { $_ -ne '' }
    $AlreadyOnPath = $PathEntries | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') }

    if (-not $AlreadyOnPath) {
        $NewPath = ($PathEntries + $InstallDir) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
        Write-Ok "Added $InstallDir to user PATH"
        Write-Info "Restart your terminal (or open a new one) for PATH to take effect."
    } else {
        Write-Info "PATH already contains $InstallDir — no change needed."
    }

} finally {
    # Clean up temp dir (T-37.1-06-02)
    if (Test-Path $TmpDir) {
        Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
    }
}

Write-Host ""
Write-Host "========================================" -ForegroundColor White
Write-Ok "IronHermes installed successfully!"
Write-Host ""
Write-Host "  Binary:  $InstallDir\$BinaryName" -ForegroundColor White
Write-Host ""
Write-Host "  Get started:" -ForegroundColor White
Write-Host "    1. Open a new terminal (so the updated PATH is active)" -ForegroundColor White
Write-Host "    2. Run: ironhermes" -ForegroundColor White
Write-Host ""
Write-Warn "update / reinstall / uninstall are not yet supported on Windows (deferred, D-12)."
Write-Warn "To update, re-run this script. To remove, delete $InstallDir and remove it from your user PATH."
Write-Host ""
