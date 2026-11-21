# Builds installer\output\MyTune-rs-Setup-<version>.exe from the release build.
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup) and the build tools listed in the README.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = ([regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"')).Groups[1].Value
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup' }

$ErrorActionPreference = 'Continue' # cargo prints warnings on stderr
# local-stickers: the Stickers tab of the chat picker, in the public build by the owner's choice (2026-11-15).
# taskbar-look: Taskbar look card, injects a DLL into explorer.exe; in the public build by the owner's choice (2026-11-20).
# license: the licence key screen (crates\mytune-core\src\license.rs); the owner's own build is made without it.
& "$root\build.ps1" build --release -p mytune-ui --features 'local-stickers,taskbar-look,license'
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
$ErrorActionPreference = 'Stop'

& $iscc "/DAppVersion=$version" installer\MyTune-rs.iss
if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
Get-Item "installer\output\MyTune-rs-Setup-$version.exe" | Select-Object FullName, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } }
