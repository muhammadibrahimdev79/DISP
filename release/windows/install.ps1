param(
    [string]$InstallDirectory = "",
    [switch]$SkipPath
)

$ErrorActionPreference = "Stop"
$expectedZigHash = "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e"
$sourceDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$dispSource = Join-Path $sourceDirectory "disp.exe"
$zigArchive = Join-Path $sourceDirectory "zig-x86_64-windows-0.16.0.zip"
$notesSource = Join-Path $sourceDirectory "RELEASE_NOTES_0.1.md"

if (-not (Test-Path -LiteralPath $dispSource -PathType Leaf)) {
    throw "The installer payload is missing disp.exe."
}
if (-not (Test-Path -LiteralPath $zigArchive -PathType Leaf)) {
    throw "The installer payload is missing the native toolchain."
}
$actualZigHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zigArchive).Hash.ToLowerInvariant()
if ($actualZigHash -ne $expectedZigHash) {
    throw "The bundled native toolchain failed its SHA-256 integrity check."
}

if ([string]::IsNullOrWhiteSpace($InstallDirectory)) {
    $InstallDirectory = Join-Path $env:LOCALAPPDATA "Programs\DISP"
}
$InstallDirectory = [IO.Path]::GetFullPath($InstallDirectory)
$parentDirectory = Split-Path -Parent $InstallDirectory
New-Item -ItemType Directory -Force -Path $parentDirectory | Out-Null

$identifier = [Guid]::NewGuid().ToString("N")
$staging = Join-Path $parentDirectory ".disp-install-$identifier"
$previous = Join-Path $parentDirectory ".disp-previous-$identifier"
New-Item -ItemType Directory -Path $staging | Out-Null

try {
    Copy-Item -LiteralPath $dispSource -Destination (Join-Path $staging "disp.exe")
    if (Test-Path -LiteralPath $notesSource -PathType Leaf) {
        Copy-Item -LiteralPath $notesSource -Destination (Join-Path $staging "RELEASE_NOTES_0.1.md")
    }
    Copy-Item -LiteralPath $MyInvocation.MyCommand.Path -Destination (Join-Path $staging "install.ps1")

    $zigStaging = Join-Path $staging ".zig"
    Expand-Archive -LiteralPath $zigArchive -DestinationPath $zigStaging
    $zigExecutable = Get-ChildItem -LiteralPath $zigStaging -Filter "zig.exe" -File -Recurse
    if ($zigExecutable.Count -ne 1) {
        throw "The native toolchain archive has an unexpected layout."
    }
    $zigRoot = $zigExecutable[0].Directory.FullName
    Move-Item -LiteralPath $zigRoot -Destination (Join-Path $staging "toolchain")
    Remove-Item -LiteralPath $zigStaging -Force -Recurse

    $version = & (Join-Path $staging "disp.exe") --version
    if ($LASTEXITCODE -ne 0 -or $version.Trim() -ne "DISP 0.1.0 Developer Preview") {
        throw "The installed compiler failed its version self-check."
    }

    if (Test-Path -LiteralPath $InstallDirectory) {
        Move-Item -LiteralPath $InstallDirectory -Destination $previous
    }
    Move-Item -LiteralPath $staging -Destination $InstallDirectory

    if (-not $SkipPath) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $entries = @($userPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
        if (-not ($entries | Where-Object { $_.TrimEnd("\") -ieq $InstallDirectory.TrimEnd("\") })) {
            $entries += $InstallDirectory
            [Environment]::SetEnvironmentVariable("Path", ($entries -join ";"), "User")
        }
    }

    if (Test-Path -LiteralPath $previous) {
        Remove-Item -LiteralPath $previous -Force -Recurse
    }
    Write-Host "DISP 0.1.0 Developer Preview installed at $InstallDirectory"
    Write-Host "Open a new terminal and run: disp --version"
} catch {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Force -Recurse
    }
    if ((Test-Path -LiteralPath $previous) -and -not (Test-Path -LiteralPath $InstallDirectory)) {
        Move-Item -LiteralPath $previous -Destination $InstallDirectory
    }
    throw
}

