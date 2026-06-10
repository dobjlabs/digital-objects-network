# Digital Objects driver installer (Windows, PowerShell 5.1+).
#
#   irm https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.ps1 | iex
#
# Installs dobj.exe (CLI), dobjd.exe (daemon), and dobj-mcp-proxy.exe from
# the latest published release into %USERPROFILE%\.dobj\bin. Pin a version:
#
#   $env:DOBJ_VERSION = "v0.1.0"; irm ... | iex
#
# No plugins are installed: the daemon starts with an empty action catalog.
# See INSTALL.md for adding plugins and connecting agents. Safe to re-run;
# re-running installs the latest release over the previous one.

$ErrorActionPreference = "Stop"

$Repo   = "dobjlabs/digital-objects-network"
$DobjHome = "$env:USERPROFILE\.dobj"
$BinDir = "$DobjHome\bin"

# --- platform ----------------------------------------------------------------

# x86_64 is the only Windows target built. Windows-on-ARM isn't supported.
if (-not [Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    Write-Error "unsupported Windows architecture: $env:PROCESSOR_ARCHITECTURE (only x86_64 is built)"
    exit 1
}
$Target = "x86_64-pc-windows-msvc"

# --- a running daemon would hold a lock on dobjd.exe; replacing it would fail
#     halfway. Bail up front with the fix instead.

$dobjdProc = Get-Process -Name "dobjd" -ErrorAction SilentlyContinue
if ($dobjdProc) {
    Write-Error "dobjd is running (pid $($dobjdProc.Id)) and Windows locks running executables. Stop it first:`n  & `"$BinDir\dobj.exe`" stop`nthen re-run this installer."
    exit 1
}

# --- release URL ---------------------------------------------------------------

if ($env:DOBJ_VERSION) {
    $Base = "https://github.com/$Repo/releases/download/$($env:DOBJ_VERSION)"
    Write-Host "installing pinned version $env:DOBJ_VERSION ($Target)"
} else {
    $Base = "https://github.com/$Repo/releases/latest/download"
    Write-Host "installing latest release ($Target)"
}

# --- download (both tarballs fully, before touching the install dir) ----------

$Tmp = Join-Path $env:TEMP "dobj-install-$([System.IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null

try {
    foreach ($name in @("dobjd", "dobj")) {
        $url = "$Base/$name-$Target.tar.gz"
        Write-Host "  fetching $name-$Target.tar.gz ..."
        # curl.exe explicitly: bare `curl` is a PowerShell alias for
        # Invoke-WebRequest with different flags.
        curl.exe -fsSL --retry 3 -o "$Tmp\$name.tar.gz" $url
        if ($LASTEXITCODE -ne 0) {
            throw "download failed: $url`n(if no release has been published yet, 'latest' does not resolve; set `$env:DOBJ_VERSION to a specific tag)"
        }
        New-Item -ItemType Directory -Force -Path "$Tmp\$name" | Out-Null
        tar -xzf "$Tmp\$name.tar.gz" -C "$Tmp\$name"
        if ($LASTEXITCODE -ne 0) { throw "extraction failed: $Tmp\$name.tar.gz" }
    }

    # --- install ---------------------------------------------------------------

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

    foreach ($name in @("dobjd", "dobj")) {
        Get-ChildItem "$Tmp\$name" -File | ForEach-Object {
            Move-Item -Force $_.FullName (Join-Path $BinDir $_.Name)
            Write-Host "  installed $BinDir\$($_.Name)"
        }
    }
}
finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

# --- report + next steps -------------------------------------------------------

Write-Host ""
$version = & "$BinDir\dobj.exe" --version
Write-Host "installed: $version"

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$BinDir*") {
    Write-Host ""
    Write-Host "to use 'dobj' without the full path, add it to your PATH (new terminals pick it up):"
    Write-Host "  [Environment]::SetEnvironmentVariable(`"Path`", `"`$([Environment]::GetEnvironmentVariable('Path','User'));$BinDir`", `"User`")"
}

Write-Host ""
Write-Host "next step: start the daemon (the first start builds ZK circuits, ~2-5 min):"
Write-Host "  & `"$BinDir\dobj.exe`" start"
Write-Host ""
Write-Host "first-run note: the binaries aren't codesigned yet, so SmartScreen may show"
Write-Host "'Windows protected your PC' -> More info -> Run anyway."
