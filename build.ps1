# Build helper for the windows-gnu Rust toolchain.
# Needs rustup (stable-x86_64-pc-windows-gnu) and WinLibs MinGW-w64 binutils
# (winget install BrechtSanders.WinLibs.POSIX.UCRT) for dlltool/as.
param([string]$Command = "build", [switch]$Release)

$winlibs = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter "BrechtSanders.WinLibs*" -ErrorAction SilentlyContinue |
    Select-Object -First 1 | ForEach-Object { Join-Path $_.FullName "mingw64\bin" }
if (-not $winlibs) { Write-Error "WinLibs not found (winget install BrechtSanders.WinLibs.POSIX.UCRT)"; exit 1 }
$env:PATH = "$env:USERPROFILE\.cargo\bin;$winlibs;$env:PATH"

$cargoArgs = @($Command)
if ($Release) { $cargoArgs += "--release" }
cargo @cargoArgs
