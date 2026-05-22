# install.ps1 — zfb Windows installer
#
# Usage:
#   irm https://raw.githubusercontent.com/Takazudo/zudo-front-builder/main/install.ps1 | iex
#
# Environment variables:
#   ZFB_VERSION   — pin a specific tag (e.g. v0.2.0) or "latest-prerelease".
#                   Default: latest non-prerelease release.
#   ZFB_INSTALL   — root install directory. Binary lands at $ZFB_INSTALL\bin\zfb.exe.
#                   Default: $env:LOCALAPPDATA\zfb

# PS 5.1 compatibility — Windows 10 ships 5.1 and users may pipe before upgrading.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Without this, Invoke-WebRequest is 10-100x slower on PS 5.x
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

# ── Architecture check ─────────────────────────────────────────────────────────
# Only x86_64 Windows is supported today. ARM64 can emulate x64 but we fail
# loudly rather than silently installing the wrong arch.
$arch = $env:PROCESSOR_ARCHITECTURE
$archWow = $env:PROCESSOR_ARCHITEW6432
if ($arch -eq 'ARM64' -or $archWow -eq 'ARM64') {
    Write-Error 'zfb: ARM64 Windows is not yet supported. Only x86_64 is available.'
    exit 1
}
$targetTriple = 'x86_64-pc-windows-msvc'

# ── Resolve version ────────────────────────────────────────────────────────────
$ghRepo = 'Takazudo/zudo-front-builder'
$requestedVersion = $env:ZFB_VERSION

if (-not $requestedVersion -or $requestedVersion -eq 'latest') {
    # Default: latest non-prerelease (GitHub excludes prereleases from /releases/latest)
    $releaseApiUrl = "https://api.github.com/repos/${ghRepo}/releases/latest"
    Write-Host "Resolving latest stable release..."
    $releaseInfo = Invoke-RestMethod -Uri $releaseApiUrl -UseBasicParsing
    $tag = $releaseInfo.tag_name
} elseif ($requestedVersion -eq 'latest-prerelease') {
    # Opt into latest prerelease.
    # Filter to actual prereleases — the naive [0] just picks the most recent
    # release of any kind, so once a stable release is published after a
    # prerelease, 'latest-prerelease' would silently resolve to that stable tag.
    $releaseApiUrl = "https://api.github.com/repos/${ghRepo}/releases?per_page=30"
    Write-Host "Resolving latest prerelease..."
    $allReleases = Invoke-RestMethod -Uri $releaseApiUrl -UseBasicParsing
    $releaseInfo = $allReleases | Where-Object { $_.prerelease -eq $true } | Select-Object -First 1
    if (-not $releaseInfo) {
        Write-Error "Could not resolve latest prerelease — no prerelease found in the last 30 releases."
        exit 1
    }
    $tag = $releaseInfo.tag_name
} else {
    # Pinned version — treat as-is (must start with "v")
    $tag = $requestedVersion
    if (-not $tag.StartsWith('v')) {
        Write-Error "ZFB_VERSION must start with 'v' (e.g. v0.2.0). Got: $tag"
        exit 1
    }
}

# Derive semver from tag (strip leading "v")
$semver = $tag.TrimStart('v')
Write-Host "Installing zfb $tag ($targetTriple)"

# ── Build asset URLs ───────────────────────────────────────────────────────────
# Archive name pattern: zfb-{semver}-{target-triple}.zip  (Windows)
# Contract locked by S3; do not change independently.
$archiveName = "zfb-${semver}-${targetTriple}.zip"
$archiveUrl = "https://github.com/${ghRepo}/releases/download/${tag}/${archiveName}"
$checksumUrl = "${archiveUrl}.sha256"

# ── Resolve install directory ──────────────────────────────────────────────────
if ($env:ZFB_INSTALL) {
    $installRoot = $env:ZFB_INSTALL
} else {
    $installRoot = Join-Path $env:LOCALAPPDATA 'zfb'
}
$binDir = Join-Path $installRoot 'bin'

# ── Download, verify, extract ─────────────────────────────────────────────────
$tmpDir = Join-Path $env:TEMP "zfb-install-$([System.Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $tmpDir | Out-Null

try {
    $archivePath = Join-Path $tmpDir $archiveName
    $checksumPath = Join-Path $tmpDir "${archiveName}.sha256"

    Write-Host "Downloading $archiveName..."
    Invoke-WebRequest -Uri $archiveUrl -OutFile $archivePath -UseBasicParsing

    Write-Host "Downloading checksum..."
    Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumPath -UseBasicParsing

    # ── Checksum verification ──────────────────────────────────────────────────
    # Checksum file format (GNU sha256sum / coreutils locked by S3):
    #   <64-hex-lowercase>  <archive-basename>
    # Parse the first whitespace-delimited field.
    # Get-FileHash returns uppercase hex — lowercase it for comparison.
    $checksumContent = (Get-Content $checksumPath -Encoding ascii).Trim()
    $expectedHash = ($checksumContent -split '\s+')[0].ToLower()

    $actualHash = (Get-FileHash $archivePath -Algorithm SHA256).Hash.ToLower()

    if ($actualHash -ne $expectedHash) {
        Write-Error "Checksum mismatch!`n  Expected: $expectedHash`n  Actual:   $actualHash"
        exit 1
    }
    Write-Host "Checksum OK."

    # ── Extract ────────────────────────────────────────────────────────────────
    $extractDir = Join-Path $tmpDir 'extract'
    New-Item -ItemType Directory -Path $extractDir | Out-Null
    Expand-Archive -Path $archivePath -DestinationPath $extractDir -Force

    # Archive root contains exactly one file: zfb.exe (contract locked by S3)
    $exeSource = Join-Path $extractDir 'zfb.exe'
    if (-not (Test-Path $exeSource)) {
        Write-Error "Archive did not contain zfb.exe at root. Archive may be corrupt."
        exit 1
    }

    # ── Install ────────────────────────────────────────────────────────────────
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
    $exeDest = Join-Path $binDir 'zfb.exe'
    Move-Item -Path $exeSource -Destination $exeDest -Force

    Write-Host ""
    Write-Host "zfb $tag installed to: $exeDest"
} finally {
    # Clean up temp directory on both success and failure
    if (Test-Path $tmpDir) {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
    }
}

# ── PATH hint ─────────────────────────────────────────────────────────────────
# Do not silently mutate user PATH — print instructions instead.
$pathEntries = $env:PATH -split ';'
if ($pathEntries -notcontains $binDir) {
    Write-Host ""
    Write-Host "Add zfb to your PATH by running ONE of the following:"
    Write-Host ""
    Write-Host "  # Current session only:"
    Write-Host "  `$env:PATH = `"$binDir;`$env:PATH`""
    Write-Host ""
    Write-Host "  # Permanently (user scope — open a new terminal after running):"
    Write-Host "  [System.Environment]::SetEnvironmentVariable('PATH', `"$binDir;`" + [System.Environment]::GetEnvironmentVariable('PATH', 'User'), 'User')"
    Write-Host ""
} else {
    Write-Host "zfb is already on your PATH."
    Write-Host ""
}

Write-Host "Run 'zfb --version' to verify the installation."
