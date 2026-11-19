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
function Install($label, $package = $installer, $tasks = '/TASKS=') {
    $log = Join-Path $PWD "dist/installer-$label.log"
    $process = Start-Process -FilePath $package -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', $tasks, "/LOG=`"$log`"") -PassThru -Wait
    Assert ($process.ExitCode -eq 0) "Installer $label succeeded"
}
New-Item -ItemType Directory -Force "$root/Data" | Out-Null
$settings = '{"AutoUpdate":false,"AutostartConfigured":true,"VoiceEndpoint":"preserve-me"}'
[IO.File]::WriteAllText("$root/Data/settings.json", $settings)

# Use the last installer whose launcher only commits current.json after the new window loads.
$legacySetup = (Resolve-Path 'dist/legacy-installer/Svyaz-0.13.0-win-x64-setup-unsigned.exe').Path

# Prepare the published runtime as a pending update, then let the genuine old EXE perform the handoff.
$seed = Start-Process -FilePath (Resolve-Path 'dist/win-x64/Связь.exe').Path -ArgumentList '--install' -Wait -PassThru
Assert ($seed.ExitCode -eq 0) 'New runtime installed into Versions'
$stateDir = Join-Path $root 'Data/AppUpdate'
$pending = Get-Content "$stateDir/current.json" -Raw | ConvertFrom-Json
Move-Item "$stateDir/current.json" "$stateDir/pending.json"
Install 'legacy' $legacySetup '/TASKS=desktopicon'
$legacy = Get-Content "$stateDir/current.json" -Raw | ConvertFrom-Json
Assert ($legacy.Version -eq '0.13.0') 'Legacy stable launcher registered'
$links = @('CommonPrograms', 'CommonDesktopDirectory') | ForEach-Object { Join-Path ([Environment]::GetFolderPath($_)) 'Связь.lnk' }
$shell = New-Object -ComObject Shell.Application
$expectedExe = [IO.Path]::GetFullPath("$root/Versions/$($pending.Id)/Связь.exe")
function IconsMigrated([switch]$ShowDetails) {
    $allMigrated = $true
    foreach ($link in $links) {
        $item = $shell.NameSpace((Split-Path $link)).ParseName([IO.Path]::GetFileName($link))
        if ($null -eq $item) {
            if ($ShowDetails) { Write-Host "Missing shortcut: $link" }
            $allMigrated = $false
            continue
        }
        [string]$iconPath = ''
        $iconIndex = $item.GetLink.GetIconLocation([ref]$iconPath)
        if ($ShowDetails) {
            $target = $item.ExtendedProperty('System.Link.TargetParsingPath')
            Write-Host "Shortcut: $link; target: $target; icon: $iconPath,$iconIndex; expected icon: $expectedExe,0"
        }
        if ($iconIndex -ne 0 -or $iconPath -ne $expectedExe) { $allMigrated = $false }
    }
    return $allMigrated
}
Assert (-not (IconsMigrated)) 'The legacy installer created shortcuts without an icon override for the new version'
[IO.File]::WriteAllText("$root/Data/settings.json", $settings.Replace('"AutoUpdate":false', '"AutoUpdate":true'))
$oldProcess = Start-Process -FilePath "$application/Связь.exe" -PassThru
try {
    Assert ($oldProcess.WaitForExit(60000) -and $oldProcess.ExitCode -eq 0) 'Previous release handed off to the new process'
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not (IconsMigrated) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
    Assert ((Get-Content "$stateDir/current.json" -Raw | ConvertFrom-Json).Id -eq $pending.Id) 'Old updater confirmed the new version'
    Assert (IconsMigrated -ShowDetails) 'New version refreshed both original installer shortcuts after legacy handoff'
}
finally {
    if (-not $oldProcess.HasExited) { $oldProcess.Kill($true); $oldProcess.WaitForExit() }
    Get-Process -Name 'Связь' -ErrorAction SilentlyContinue | Where-Object Path -eq $expectedExe | ForEach-Object { $_.Kill($true); $_.WaitForExit() }
    [IO.File]::WriteAllText("$root/Data/settings.json", $settings)
}
Install first
$current = Get-Content "$root/Data/AppUpdate/current.json" -Raw | ConvertFrom-Json
Assert ($current.Version -eq $version) 'Installer registered version for existing shortcuts and autostart'
Assert (Test-Path "$root/Versions/$($current.Id)/Связь.exe") 'Versioned runtime installed'
$shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Связь.lnk'
Assert (Test-Path $shortcut) 'Start menu shortcut installed'
$shortcutItem = $shell.NameSpace((Split-Path $shortcut)).ParseName([IO.Path]::GetFileName($shortcut))
$shortcutTarget = $shortcutItem.ExtendedProperty('System.Link.TargetParsingPath')
Write-Host "Shortcut target: $shortcutTarget; expected: $application\Связь.exe"
Copy-Item $shortcut dist/installer-shortcut.lnk
Assert (-not [string]::IsNullOrEmpty($shortcutTarget)) 'Shortcut has a filesystem target'
Assert ([IO.Path]::GetFullPath($shortcutTarget) -eq [IO.Path]::GetFullPath("$application\Связь.exe")) 'Shortcut targets stable application path'
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
    Assert ([IO.Path]::GetFullPath($migrated.Actions.Execute) -eq [IO.Path]::GetFullPath("$root/Versions/$($current.Id)/Связь.exe")) 'Legacy autostart migrated without opening app'
    Assert ($migrated.Actions.Arguments -eq '--startup' -and $migrated.State -eq 'Disabled') 'Autostart arguments and disabled state preserved'
    $uninstaller = (Get-ChildItem $root -Filter 'unins*.exe' | Select-Object -First 1).FullName
    $log = Join-Path $PWD 'dist/installer-uninstall.log'
    Copy-Item "$root/Data/AppUpdate/current.json" "$root/Data/AppUpdate/rollback.json"
    $process = Start-Process $uninstaller -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") -PassThru -Wait
    Assert ($process.ExitCode -eq 0) 'Uninstaller succeeded'
    Assert (-not (Test-Path "$application/Связь.exe")) 'Application removed'
    Assert (-not (Test-Path "$root/Versions")) 'Downloaded app versions removed'
    Assert (-not (Test-Path $shortcut)) 'Shortcut removed'
    Assert ($null -eq (Get-ScheduledTask -TaskName $ownName -ErrorAction SilentlyContinue)) 'Owned autostart removed'
    Assert ($null -ne (Get-ScheduledTask -TaskName $foreignName -ErrorAction SilentlyContinue)) 'Unrelated task preserved'
    Assert ([IO.File]::ReadAllText("$root/Data/settings.json") -ceq $settings) 'Uninstall preserves settings exactly'
    Assert (-not (Test-Path "$root/Data/AppUpdate/rollback.json")) 'Uninstall clears rollback pointer'
    Install reinstall
    Assert ([IO.File]::ReadAllText("$root/Data/settings.json") -ceq $settings) 'Reinstall preserves settings exactly'
}
finally {
    foreach ($name in @($ownName, $foreignName, 'DiscordRoute-CI-legacy')) {
        if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) { Unregister-ScheduledTask -TaskName $name -Confirm:$false }
    }
}
