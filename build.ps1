#requires -Version 7.4
[CmdletBinding()]
param(
    [string]$CertificateThumbprint,
    [string]$SignToolPath,
    [uri]$TimestampUrl,
    [string]$IsccPath = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
)
$ErrorActionPreference = 'Stop'
if (($CertificateThumbprint -or $SignToolPath -or $TimestampUrl) -and
    -not ($CertificateThumbprint -and $SignToolPath -and $TimestampUrl)) {
    throw 'Для подписи нужны CertificateThumbprint, SignToolPath и TimestampUrl вместе.'
}
if (-not $IsWindows) { throw 'Сборка Windows x64 выполняется в Windows.' }
Push-Location $PSScriptRoot
try {
    & ./tests/Signing.Checks.ps1
    cargo test --workspace --locked --release --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Проверки Rust завершились с ошибкой.' }
    & ./scripts/build-rust.ps1 @PSBoundParameters
} finally { Pop-Location }
