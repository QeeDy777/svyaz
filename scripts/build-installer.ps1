#requires -Version 7.4
[CmdletBinding()]
param([string]$IsccPath = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe")
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Установщик собирается на Windows с Inno Setup 6.' }
Push-Location (Join-Path $PSScriptRoot '..')
try {
    $stage = Join-Path $PWD 'dist/rust-candidate'
    $exe = Join-Path $stage 'Связь.exe'
    if (-not (Test-Path -LiteralPath $exe)) { throw 'Сначала выполните ./build.ps1.' }
    $info = (Get-Item -LiteralPath $exe).VersionInfo
    $version = "$($info.FileMajorPart).$($info.FileMinorPart).$($info.FileBuildPart)"
    # Signing is a separate release step; never label an unsigned installer as signed.
    & $IsccPath "/DAppVersion=$version" "/DAppFiles=$stage" /DVariant=unsigned installer/Svyaz.iss
    if ($LASTEXITCODE -ne 0) { throw 'Сборка установщика не прошла.' }
    $file = "dist/Svyaz-$version-win-x64-setup-unsigned.exe"
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($file))" | Set-Content "$file.sha256" -Encoding utf8NoBOM
    Write-Host "Готово: $file"
}
finally { Pop-Location }
