#requires -Version 7.4
[CmdletBinding()]
param(
    [string]$CertificateThumbprint,
    [string]$SignToolPath,
    [uri]$TimestampUrl
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (($CertificateThumbprint -or $SignToolPath -or $TimestampUrl) -and
    -not ($CertificateThumbprint -and $SignToolPath -and $TimestampUrl)) {
    throw 'Для подписи нужны CertificateThumbprint, SignToolPath и TimestampUrl вместе.'
}
Push-Location $PSScriptRoot
try {
    $version = ([xml](Get-Content Directory.Build.props -Raw)).Project.PropertyGroup.Version
    $variant = if ($CertificateThumbprint) { 'signed' } else { 'unsigned' }
    $archive = "dist/Svyaz-$version-win-x64-$variant.zip"
    & ./tests/Signing.Checks.ps1
    dotnet run --project tests/DiscordRoute.Checks
    if ($LASTEXITCODE -ne 0) { throw 'Проверки не прошли.' }
    if (Test-Path dist/win-x64) { Remove-Item dist/win-x64 -Recurse -Force }
    python scripts/prepare-telegram.py
    if ($LASTEXITCODE -ne 0) { throw 'Сборка встроенного Telegram не прошла.' }
    dotnet publish src/DiscordRoute.App -c Release -r win-x64 --self-contained true -o dist/win-x64
    if ($LASTEXITCODE -ne 0) { throw 'Сборка не прошла.' }
    Get-ChildItem dist/win-x64 -Filter '*.pdb' | Remove-Item
    if ($CertificateThumbprint) {
        & ./scripts/sign-release.ps1 -CertificateThumbprint $CertificateThumbprint -SignToolPath $SignToolPath -TimestampUrl $TimestampUrl
    }
    Copy-Item README.md dist/win-x64/README.txt -Force
    Copy-Item VALIDATION.md dist/win-x64/VALIDATION.txt -Force
    Copy-Item SIGNING.md dist/win-x64/SIGNING.txt -Force
    Compress-Archive -Path dist/win-x64/* -DestinationPath $archive -Force
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($archive))" | Set-Content "$archive.sha256" -Encoding utf8NoBOM
    Write-Host "Готово: $archive"
    if (-not $CertificateThumbprint) { Write-Host 'EXE не подписан. Windows может показать SmartScreen.' }
}
finally { Pop-Location }
