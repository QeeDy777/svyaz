#requires -Version 7.4
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$checks = 0
function Check([bool]$condition, [string]$name) {
    if (-not $condition) { throw "FAIL: $name" }
    $script:checks++
    Write-Host "PASS: $name"
}
function ExpectFailure([scriptblock]$action, [string]$name) {
    $failed = $false
    try { & $action } catch { $failed = $true }
    Check $failed $name
}
foreach ($path in @('build.ps1', 'scripts/sign-release.ps1')) {
    $parseErrors = $null
    [void][Management.Automation.Language.Parser]::ParseFile((Join-Path $root $path), [ref]$null, [ref]$parseErrors)
    Check ($parseErrors.Count -eq 0) "PowerShell syntax: $path"
}
$temp = Join-Path ([IO.Path]::GetTempPath()) ('discord-sign-check-' + [guid]::NewGuid().ToString('N'))
New-Item $temp -ItemType Directory | Out-Null
try {
    Set-Content (Join-Path $temp 'Связь.exe') 'TEST FIXTURE, NOT A PROGRAM'
    $tool = Join-Path $temp 'FakeSignTool.ps1'
    @'
$global:SignCalls.Add(@($args))
$global:LASTEXITCODE = if ($args[0] -eq 'sign') { $global:SignExit } else { $global:VerifyExit }
'@ | Set-Content $tool
    $global:SignCalls = [Collections.Generic.List[object]]::new()
    $global:SignExit = 0
    $global:VerifyExit = 0
    $thumbprint = 'A' * 40
    $global:TestSignature = [pscustomobject]@{
        Status = 'Valid'
        TimeStamperCertificate = [pscustomobject]@{ Subject = 'TEST TIMESTAMP' }
        SignerCertificate = [pscustomobject]@{ Thumbprint = $thumbprint }
    }
    # Test doubles exercise orchestration only; no real certificate is used.
    function global:Get-AuthenticodeSignature { param($LiteralPath) $global:TestSignature }
    $parameters = @{
        CertificateThumbprint = $thumbprint
        SignToolPath = $tool
        TimestampUrl = 'https://timestamp.example.test'
        PublishDirectory = $temp
    }
    $sign = Join-Path $root 'scripts/sign-release.ps1'
    & $sign @parameters
    Check ($global:SignCalls.Count -eq 2 -and $global:SignCalls[0][0] -eq 'sign' -and $global:SignCalls[1][0] -eq 'verify') 'Sign before verify'
    Check ($global:SignCalls[0] -contains 'SHA256' -and $global:SignCalls[0] -contains '/tr' -and $global:SignCalls[1] -contains '/tw') 'SHA-256 and timestamp required'
    $global:SignExit = 1
    ExpectFailure { & $sign @parameters } 'Reject signing warning or failure'
    $global:SignExit = 0; $global:VerifyExit = 1
    ExpectFailure { & $sign @parameters } 'Reject failed verification'
    $global:VerifyExit = 0
    $global:TestSignature.Status = 'NotTrusted'
    ExpectFailure { & $sign @parameters } 'Reject untrusted signature'
    $global:TestSignature.Status = 'Valid'; $global:TestSignature.TimeStamperCertificate = $null
    ExpectFailure { & $sign @parameters } 'Reject missing timestamp'
    $global:TestSignature.TimeStamperCertificate = [pscustomobject]@{}
    $global:TestSignature.SignerCertificate.Thumbprint = 'B' * 40
    ExpectFailure { & $sign @parameters } 'Reject different publisher certificate'
    ExpectFailure { & (Join-Path $root 'build.ps1') -CertificateThumbprint $thumbprint } 'Reject partial signing configuration before build'
    Remove-Item (Join-Path $temp 'Связь.exe')
    ExpectFailure { & $sign @parameters } 'Reject missing published executable'
}
finally {
    Remove-Item $temp -Recurse -Force
    Remove-Item Function:\Get-AuthenticodeSignature
    Remove-Variable SignCalls, SignExit, VerifyExit, TestSignature -Scope Global
}
Write-Host "$checks signing orchestration checks passed (test doubles; no real signature)."
