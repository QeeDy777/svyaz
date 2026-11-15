#requires -Version 7.4
[CmdletBinding()]
param([string]$IsccPath = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe")
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Установщик собирается на Windows с Inno Setup 6.' }
Push-Location (Join-Path $PSScriptRoot '..')
try {
    if (-not (Test-Path dist/win-x64/Связь.exe)) { throw 'Сначала выполните ./build.ps1.' }
    $version = ([xml](Get-Content Directory.Build.props -Raw)).Project.PropertyGroup.Version
    if ((Get-Item dist/win-x64/Связь.exe).VersionInfo.ProductVersion.Split('+')[0] -ne $version) {
        throw 'Версия опубликованного EXE отличается от версии проекта.'
    }
    # Signing is a separate release step; never label an unsigned installer as signed.
    & $IsccPath "/DAppVersion=$version" /DVariant=unsigned installer/Svyaz.iss
    if ($LASTEXITCODE -ne 0) { throw 'Сборка установщика не прошла.' }
    $file = "dist/Svyaz-$version-win-x64-setup-unsigned.exe"
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($file))" | Set-Content "$file.sha256" -Encoding utf8NoBOM
    Write-Host "Готово: $file"
}
finally { Pop-Location }
