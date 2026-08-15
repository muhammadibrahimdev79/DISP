[CmdletBinding()]
param(
    [ValidateSet("check", "test", "verify", "example")]
    [string]$Task = "check",
    [string]$Example = "examples/hello.disp",
    [string]$TargetDirectory = ""
)

$ErrorActionPreference = "Stop"
$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repository = [IO.Path]::GetFullPath((Join-Path $scriptDirectory ".."))
$compiler = Join-Path $repository "compiler"
$manifest = Join-Path $compiler "Cargo.toml"

if ([string]::IsNullOrWhiteSpace($TargetDirectory)) {
    $cacheRoot = if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        Join-Path $repository ".targets"
    } else {
        Join-Path $env:LOCALAPPDATA "DISP"
    }
    $TargetDirectory = Join-Path $cacheRoot "cargo-target"
}

$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
New-Item -ItemType Directory -Force -Path $TargetDirectory | Out-Null
$previousTargetDirectory = $env:CARGO_TARGET_DIR
$previousBuildRoot = $env:DISP_BUILD_ROOT
$env:CARGO_TARGET_DIR = $TargetDirectory

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Program,
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program failed with exit code $LASTEXITCODE"
    }
}

function Invoke-FormatCheck {
    Invoke-Checked "cargo" @("fmt", "--manifest-path", $manifest, "--check")
}

function Invoke-CompilerCheck {
    Invoke-Checked "cargo" @("check", "--manifest-path", $manifest, "--all-targets")
}

function Invoke-Tests {
    Invoke-Checked "cargo" @("test", "--manifest-path", $manifest, "--", "--test-threads=1")
}

Push-Location $compiler
try {
    Write-Host "DISP task: $Task"
    Write-Host "Shared Cargo cache: $TargetDirectory"

    switch ($Task) {
        "check" {
            Invoke-FormatCheck
            Invoke-CompilerCheck
        }
        "test" {
            Invoke-Tests
        }
        "example" {
            $env:DISP_BUILD_ROOT = Join-Path $TargetDirectory "native"
            Invoke-Checked "cargo" @(
                "run",
                "--manifest-path", $manifest,
                "--quiet",
                "--",
                "run", $Example
            )
        }
        "verify" {
            Invoke-FormatCheck
            Invoke-Checked "cargo" @(
                "clippy",
                "--manifest-path", $manifest,
                "--all-targets",
                "--",
                "-D", "warnings"
            )
            Invoke-Tests
            Invoke-Checked "cargo" @("build", "--manifest-path", $manifest)
            Invoke-Checked "git" @("-C", $repository, "diff", "--check")
        }
    }
} finally {
    Pop-Location
    $env:CARGO_TARGET_DIR = $previousTargetDirectory
    $env:DISP_BUILD_ROOT = $previousBuildRoot
}
