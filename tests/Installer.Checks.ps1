#requires -Version 7.4
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true') { throw 'Runs only on a disposable Windows CI runner.' }
$root = Join-Path $env:ProgramFiles DiscordRoute
$application = Join-Path $root Application
$version = ([xml](Get-Content Directory.Build.props -Raw)).Project.PropertyGroup.Version
$installer = (Resolve-Path "dist/Svyaz-$version-win-x64-setup-unsigned.exe").Path
if ((Test-Path $application) -or (Test-Path "$root/Versions") -or (Test-Path "$root/Data/settings.json")) {
    throw 'Refusing to modify an existing installation.'
}
function Assert($condition, $message) {
    if (-not $condition) { throw $message }
    Write-Host "PASS: $message"
}
function Install($label) {
    $log = Join-Path $PWD "dist/installer-$label.log"
    $process = Start-Process -FilePath $installer -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") -PassThru -Wait
    Assert ($process.ExitCode -eq 0) "Installer $label succeeded"
}
New-Item -ItemType Directory -Force "$root/Data" | Out-Null
$settings = '{"AutoUpdate":false,"AutostartConfigured":true,"VoiceEndpoint":"preserve-me"}'
[IO.File]::WriteAllText("$root/Data/settings.json", $settings)
Install first
$current = Get-Content "$root/Data/AppUpdate/current.json" -Raw | ConvertFrom-Json
Assert ($current.Version -eq $version) 'Installer registered version for existing shortcuts and autostart'
Assert (Test-Path "$root/Versions/$($current.Id)/Связь.exe") 'Versioned runtime installed'
$shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Связь.lnk'
Assert (Test-Path $shortcut) 'Start menu shortcut installed'
$shell = New-Object -ComObject WScript.Shell
Assert ($shell.CreateShortcut($shortcut).TargetPath -eq "$application\Связь.exe") 'Shortcut targets stable application path'
Remove-Item "$root/Versions/$($current.Id)/wpfgfx_cor3.dll"
Install repair
Assert (Test-Path "$root/Versions/$($current.Id)/wpfgfx_cor3.dll") 'Repair restores missing runtime DLL'
Assert ([IO.File]::ReadAllText("$root/Data/settings.json") -ceq $settings) 'Repair preserves settings exactly'
$ownName = 'DiscordRoute-CI-owned'
$foreignName = 'DiscordRoute-CI-foreign'
try {
    Register-ScheduledTask -TaskName $ownName -Action (New-ScheduledTaskAction -Execute "$root/Versions/$($current.Id)/Связь.exe") -Force | Out-Null
    Register-ScheduledTask -TaskName $foreignName -Action (New-ScheduledTaskAction -Execute "$env:SystemRoot/System32/notepad.exe") -Force | Out-Null
    $legacyName = 'DiscordRoute-CI-legacy'
    Register-ScheduledTask -TaskName $legacyName -Action (New-ScheduledTaskAction -Execute "$root/Versions/old/Связь.exe" -Argument '--startup') -Trigger (New-ScheduledTaskTrigger -AtLogOn) -Force | Out-Null
    Disable-ScheduledTask -TaskName $legacyName | Out-Null
    Install migrate
    $migrated = Get-ScheduledTask -TaskName $legacyName
    Assert ($migrated.Actions.Execute -eq "$root/Versions/$($current.Id)/Связь.exe") 'Legacy autostart migrated without opening app'
    Assert ($migrated.Actions.Arguments -eq '--startup' -and $migrated.State -eq 'Disabled') 'Autostart arguments and disabled state preserved'
    $uninstaller = (Get-ChildItem $root -Filter 'unins*.exe' | Select-Object -First 1).FullName
    $log = Join-Path $PWD 'dist/installer-uninstall.log'
    $process = Start-Process $uninstaller -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") -PassThru -Wait
    Assert ($process.ExitCode -eq 0) 'Uninstaller succeeded'
    Assert (-not (Test-Path "$application/Связь.exe")) 'Application removed'
    Assert (-not (Test-Path "$root/Versions")) 'Downloaded app versions removed'
    Assert (-not (Test-Path $shortcut)) 'Shortcut removed'
    Assert ($null -eq (Get-ScheduledTask -TaskName $ownName -ErrorAction SilentlyContinue)) 'Owned autostart removed'
    Assert ($null -ne (Get-ScheduledTask -TaskName $foreignName -ErrorAction SilentlyContinue)) 'Unrelated task preserved'
    Assert ([IO.File]::ReadAllText("$root/Data/settings.json") -ceq $settings) 'Uninstall preserves settings exactly'
    Install reinstall
    Assert ([IO.File]::ReadAllText("$root/Data/settings.json") -ceq $settings) 'Reinstall preserves settings exactly'
}
finally {
    foreach ($name in @($ownName, $foreignName, 'DiscordRoute-CI-legacy')) {
        if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) { Unregister-ScheduledTask -TaskName $name -Confirm:$false }
    }
}
