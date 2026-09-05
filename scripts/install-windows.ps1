[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:USERPROFILE '.local'),
    [string]$GnuBin = '',
    [switch]$Update,
    [string]$Branch = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$source = Split-Path -Parent $PSScriptRoot
$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
$installBin = Join-Path $InstallRoot 'bin'

function Invoke-Checked {
    param([string]$Program, [string[]]$Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE." }
}

Push-Location -LiteralPath $source
try {
    if ($Update) {
        $dirty = & git -C $source status --porcelain
        if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect the source checkout.' }
        if ($dirty) { throw 'The kanspec source checkout has local changes. Commit or resolve them before updating.' }
        if ($Branch) { Invoke-Checked git @('-C', $source, 'switch', $Branch) }
        Invoke-Checked git @('-C', $source, 'pull', '--ff-only')
    }

    $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
    $cargoPath = if ($cargo) { $cargo.Source } else { Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
    if (-not (Test-Path -LiteralPath $cargoPath)) {
        throw 'Install Rust with rustup first: https://rust-lang.org/tools/install/'
    }
    $env:PATH = (Split-Path -Parent $cargoPath) + ';' + $env:PATH
    if ($GnuBin) {
        $GnuBin = [IO.Path]::GetFullPath($GnuBin)
        $gcc = Join-Path $GnuBin 'gcc.exe'
        if (-not (Test-Path -LiteralPath $gcc)) { throw "GCC was not found at $gcc." }
        $env:PATH = $GnuBin + ';' + $env:PATH
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $gcc
    }

    # Force a rebuild even while the package version stays unchanged; build.rs stamps
    # the checkout revision into --version. Cargo replaces the installed executables
    # only after a successful build, so a failed update leaves the previous build usable.
    Invoke-Checked $cargoPath @('install', '--locked', '--path', $source, '--root', $InstallRoot, '--bins', '--force')
    Invoke-Checked (Join-Path $installBin 'kanspec.exe') @('--version')
    Invoke-Checked (Join-Path $installBin 'ks.exe') @('--version')

    # Keep this command tied to the source installation. It accepts extra parameters,
    # e.g. kanspec-update -Branch main after switching the source to the main branch.
    foreach ($path in @($source, $installBin, $GnuBin, $InstallRoot)) {
        if ($path -match '[%"\r\n]') { throw 'The update wrapper requires paths without percent signs, quotes, or newlines.' }
    }
    $scriptPath = Join-Path $PSScriptRoot 'install-windows.ps1'
    # Reuse the PowerShell runtime that installed us, including its script policy.
    $shellName = if ($PSVersionTable.PSEdition -eq 'Core') { 'pwsh.exe' } else { 'powershell.exe' }
    $shellPath = Join-Path $PSHOME $shellName
    $wrapper = "@echo off`r`nsetlocal`r`n" +
        '"' + $shellPath + '" -NoProfile -File "' + $scriptPath +
        '" -Update -InstallRoot "' + $InstallRoot + '"'
    if ($GnuBin) { $wrapper += ' -GnuBin "' + $GnuBin + '"' }
    $wrapper += " %*`r`nexit /b %errorlevel%`r`n"
    [IO.File]::WriteAllText((Join-Path $installBin 'kanspec-update.cmd'), $wrapper, [Text.Encoding]::Default)

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if (-not ($entries | Where-Object { $_.TrimEnd('\', '/') -ieq $installBin.TrimEnd('\', '/') })) {
        $updatedPath = (@($entries) + $installBin) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $updatedPath, 'User')
    }
    $env:PATH = $installBin + ';' + $env:PATH
    Write-Host "Installed kanspec and ks in $installBin."
    Write-Host 'Run kanspec-update to pull the current source branch and rebuild in place.'
    Write-Host 'New terminals inherit the saved user PATH; restart an existing app if it lacks that directory.'
} finally {
    Pop-Location
}
