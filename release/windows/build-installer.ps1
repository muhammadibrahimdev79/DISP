param(
    [Parameter(Mandatory = $true)]
    [string]$ZigArchive,
    [string]$OutputPath = ""
)

$ErrorActionPreference = "Stop"
$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repository = [IO.Path]::GetFullPath((Join-Path $scriptDirectory "..\.."))
$compiler = Join-Path $repository "compiler"
$expectedZigHash = "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e"
$ZigArchive = [IO.Path]::GetFullPath($ZigArchive)
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $ZigArchive).Hash.ToLowerInvariant() -ne $expectedZigHash) {
    throw "Zig archive SHA-256 does not match the pinned official 0.16.0 release."
}
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $repository "DISP-0.1-windows-x64.exe"
}
$OutputPath = [IO.Path]::GetFullPath($OutputPath)

Push-Location $compiler
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
} finally {
    Pop-Location
}

$env:DISP_COMPILER_EXE = Join-Path $compiler "target\release\disp.exe"
$env:DISP_ZIG_ARCHIVE = $ZigArchive
$env:DISP_INSTALL_SCRIPT = Join-Path $scriptDirectory "install.ps1"
$env:DISP_RELEASE_NOTES = Join-Path $repository "docs\releases\RELEASE_NOTES_0.1.md"
try {
    rustc (Join-Path $scriptDirectory "bootstrap.rs") -O -o $OutputPath
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $OutputPath -PathType Leaf)) {
        throw "rustc failed to create the DISP installer."
    }
} finally {
    Remove-Item Env:DISP_COMPILER_EXE -ErrorAction SilentlyContinue
    Remove-Item Env:DISP_ZIG_ARCHIVE -ErrorAction SilentlyContinue
    Remove-Item Env:DISP_INSTALL_SCRIPT -ErrorAction SilentlyContinue
    Remove-Item Env:DISP_RELEASE_NOTES -ErrorAction SilentlyContinue
}

Write-Host "Created $OutputPath"
Get-FileHash -Algorithm SHA256 -LiteralPath $OutputPath
