#requires -Version 7.4
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{40}$')][string]$CertificateThumbprint,
    [Parameter(Mandatory)][string]$SignToolPath,
    [Parameter(Mandatory)][uri]$TimestampUrl,
    [string]$PublishDirectory = (Join-Path $PSScriptRoot '../dist/win-x64')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($TimestampUrl.Scheme -notin @('http', 'https') -or -not $TimestampUrl.IsAbsoluteUri) {
    throw 'Укажите HTTP(S) URL службы RFC 3161 вашего удостоверяющего центра.'
}
$tool = (Resolve-Path -LiteralPath $SignToolPath).Path
$directory = (Resolve-Path -LiteralPath $PublishDirectory).Path
$exe = Join-Path $directory 'Связь.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Сначала соберите Связь.exe.' }
# The private key stays in the Windows certificate store / hardware provider.
& $tool sign /s My /sha1 $CertificateThumbprint /fd SHA256 /tr $TimestampUrl.AbsoluteUri /td SHA256 /d Связь $exe
if ($LASTEXITCODE -ne 0) { throw 'SignTool: подпись или timestamp не созданы. Архив не выпускать.' }
& $tool verify /pa /all /v /tw $exe
if ($LASTEXITCODE -ne 0) { throw 'SignTool: проверка подписи не пройдена. Архив не выпускать.' }
$signature = Get-AuthenticodeSignature -LiteralPath $exe
if ($signature.Status -ne 'Valid' -or $null -eq $signature.TimeStamperCertificate -or
    $signature.SignerCertificate.Thumbprint -ne $CertificateThumbprint) {
    throw 'Не подтверждены доверенная подпись выбранного издателя и timestamp.'
}
Write-Host 'Подпись проверена. Теперь можно упаковать релиз и рассчитать SHA-256.'
