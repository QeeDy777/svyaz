#requires -Version 7.4
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $NativeInstaller,
    [string] $CandidateDirectory = 'dist/rust-candidate',
    [string] $LegacyInstaller = 'dist/rust-compat/Svyaz-0.13.3-win-x64-setup-unsigned.exe'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true') { throw 'Use a disposable Windows CI runner.' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'An elevated CI account is required.' }

$root = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) DiscordRoute
$data = Join-Path $root Data
$app = Join-Path $root 'Application/Связь.exe'
$update = Join-Path $data AppUpdate
$preferences = Join-Path $data preferences.json
$settings = Join-Path $data settings.json
$nativeSetup = (Resolve-Path -LiteralPath $NativeInstaller).Path
$oldSetup = (Resolve-Path -LiteralPath $LegacyInstaller).Path
$candidate = (Resolve-Path -LiteralPath (Join-Path $CandidateDirectory 'Связь.exe')).Path
$info = [Diagnostics.FileVersionInfo]::GetVersionInfo($candidate)
$version = [Version]::new($info.FileMajorPart, $info.FileMinorPart, $info.FileBuildPart)
$links = 'CommonPrograms', 'CommonDesktopDirectory' | ForEach-Object { Join-Path ([Environment]::GetFolderPath($_)) 'Связь.lnk' }
$ownTask = 'DiscordRoute-RustMigration-CI-owned'
$foreignTask = 'DiscordRoute-RustMigration-CI-foreign'
if ((Test-Path $root) -or ($links | Where-Object { Test-Path -LiteralPath $_ }) -or
    (Get-ScheduledTask -TaskName $ownTask,$foreignTask -ErrorAction SilentlyContinue)) {
    throw 'A clean runner without an existing DiscordRoute installation, links or test tasks is required.'
}
function Assert($value, [string] $message) {
    if (-not $value) { throw $message }
    Write-Host "PASS: $message"
}
function Run-Checked([string]$file, [string[]]$arguments, [int]$seconds = 90) {
    $p = Start-Process -FilePath $file -ArgumentList $arguments -PassThru
    if (-not $p.WaitForExit($seconds * 1000)) {
        $p.Kill($true)
        $p.WaitForExit()
        throw "Timed out waiting for $file ($arguments)"
    }
    if ($p.ExitCode -ne 0) { throw "$file exited with $($p.ExitCode)" }
}
function Install-Package([string]$file, [string]$label) {
    $log = Join-Path $PWD "dist/rust-installer-$label.log"
    Run-Checked $file @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/TASKS=desktopicon',"/LOG=`"$log`"")
}
function Pointer([string]$name) { Get-Content (Join-Path $update "$name.json") -Raw | ConvertFrom-Json }
function Executable($pointer) { [IO.Path]::GetFullPath("$root/Versions/$($pointer.Id)/Связь.exe") }
function Stop-ExactProcess([string]$path) {
    Get-Process -Name 'Связь' -ErrorAction SilentlyContinue | Where-Object Path -eq $path | ForEach-Object {
        if (-not $_.HasExited) { $_.Kill($true); $_.WaitForExit() }
    }
}
function Save-NativeTabs([Diagnostics.Process]$runtime) {
    $script:NativeWindowClass = & $windowCheck capture $runtime.Id ($runtime.MainWindowHandle.ToInt64()) (Join-Path $PWD dist)
    Assert ($LASTEXITCODE -eq 0 -and $script:NativeWindowClass) 'Captured all four rendered tabs and their readable titles on the fixture window'
}
function Assert-Runtime($item,[bool]$native,[bool]$captureTabs=$true) {
    $exe = Executable $item
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        $runtime = Get-Process -Name 'Связь' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe -and $_.MainWindowHandle -ne 0 } | Select-Object -First 1
        if ($runtime) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    Assert ($null -ne $runtime) "Confirmed version $($item.Version) has a visible application window"
    if ($native) {
        $managedModules = @($runtime.Modules | Where-Object { $_.ModuleName -like 'python*.dll' -or $_.ModuleName -in @('coreclr.dll','wpfgfx_cor3.dll','PresentationNative_cor3.dll') })
        Assert ($managedModules.Count -eq 0) 'The candidate runs without loading CPython, the CLR or WPF'
        if ($captureTabs) { Save-NativeTabs $runtime }
    }
}
function Assert-Links($item) {
    $explorer = New-Object -ComObject Shell.Application
    foreach ($link in $links) {
        $itemLink = $explorer.NameSpace((Split-Path $link)).ParseName([IO.Path]::GetFileName($link))
        Assert ($null -ne $itemLink) "Installer shortcut exists at $link"
        [string]$icon = ''
        $index = $itemLink.GetLink.GetIconLocation([ref]$icon)
        $target = $itemLink.ExtendedProperty('System.Link.TargetParsingPath')
        Assert ($target -and [IO.Path]::GetFullPath($target) -eq [IO.Path]::GetFullPath($app)) 'Shortcut still opens the stable launcher'
        Assert ($index -eq 0 -and $icon -eq (Executable $item)) "Shortcut shows the icon of $($item.Version)"
    }
}
function Start-Handoff([string]$launcher) {
    $parent = Start-Process -FilePath $launcher -PassThru
    if (-not $parent.WaitForExit(60000)) {
        $parent.Kill($true)
        $parent.WaitForExit()
        throw 'Launcher did not complete the handoff'
    }
    Assert ($parent.ExitCode -eq 0) 'Parent launcher committed a successful handoff'
}
function Assert-TrayStartup([string]$nativeExe) {
    # Isolate tray behavior from the network services; exercise the same child readiness event used in production.
    $savedSettings = [IO.File]::ReadAllBytes($settings)
    $savedPrefs = [IO.File]::ReadAllBytes($preferences)
    $token = [guid]::NewGuid().ToString('N')
    $ready = [Threading.EventWaitHandle]::new($false,[Threading.EventResetMode]::ManualReset,"Local\Svyaz-Update-$token")
    try {
        [IO.File]::WriteAllText($settings,$settingsSeed)
        [IO.File]::WriteAllText($preferences,'{"Services":{"Discord":false,"YouTube":false,"Telegram":false},"QuietNotifications":true}')
        $runtime = Start-Process $nativeExe -ArgumentList @('--startup','--parent-refreshes-shortcut-icons','--update-ready',$token) -PassThru
        Assert ($ready.WaitOne(20000)) 'The logon runtime completed its real UI initialization'
        & $windowCheck hidden $runtime.Id $script:NativeWindowClass
        $exit = $LASTEXITCODE
        $runtime.Refresh()
        Assert ($exit -eq 0 -and -not $runtime.HasExited) 'The ready logon runtime keeps its main window hidden'
    } finally {
        Stop-ExactProcess $nativeExe
        $ready.Dispose()
        [IO.File]::WriteAllBytes($settings,$savedSettings)
        [IO.File]::WriteAllBytes($preferences,$savedPrefs)
    }
}

# Resolve the exact test executable from Cargo, including custom target-directory configurations.
$build = cargo build --locked --release --target x86_64-pc-windows-msvc -p svyaz-windows --example migration-ui --message-format=json-render-diagnostics
if ($LASTEXITCODE) { throw 'Could not build the Windows desktop test helper.' }
$windowCheck = $build | ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'migration-ui' -and $_.executable } |
    Select-Object -ExpandProperty executable -Last 1
Assert ($windowCheck -and (Test-Path -LiteralPath $windowCheck)) 'Located the native Windows desktop test helper'

Assert ($version -gt [Version]'0.13.3') 'Native EXE has an updatable numeric Windows file version'
$null = New-Item -ItemType Directory -Force $data
$settingsSeed = '{"AutoUpdate":false,"AutostartConfigured":true,"LastStrategy":"migration-fixture","Telegram":{"Port":32412,"Secret":"","StartWithApp":false}}'
$prefsSeed = '{"NetworkStrategies":{"migration-network":"migration-alt"},"Services":{"Discord":true,"YouTube":true,"Telegram":false},"QuietNotifications":true}'
[IO.File]::WriteAllText($settings,$settingsSeed)
[IO.File]::WriteAllText($preferences,$prefsSeed)
$knownExecutables = @($app)
$staged = $null
try {
    # Start the copy from the portable archive, with the 0.13.3 entry allowlist and size bounds.
    $zipPath = (Resolve-Path "dist/Svyaz-$version-win-x64-unsigned.zip").Path
    $sha = ([IO.File]::ReadAllText("$zipPath.sha256").Trim() -split '\s+')[0]
    Assert ((Get-FileHash $zipPath).Hash -eq $sha) 'Portable archive matches its published checksum'
    $runtimeFiles = 'Связь.exe','D3DCompiler_47_cor3.dll','PenImc_cor3.dll','PresentationNative_cor3.dll','vcruntime140_cor3.dll','wpfgfx_cor3.dll'
    $allowed = $runtimeFiles + @('README.txt','VALIDATION.txt','SIGNING.txt','DOTNET-LICENSE.txt','DOTNET-THIRD-PARTY-NOTICES.txt','WINDOWS-DESKTOP-LICENSE.txt','TG-WS-PROXY-LICENSE.txt')
    $staged = Join-Path $PWD "dist/rust-migration-stage-$([guid]::NewGuid().ToString('N'))"
    $null = New-Item -ItemType Directory $staged
    $zip = [IO.Compression.ZipFile]::OpenRead($zipPath)
    try {
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        [long]$size = 0
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName -cnotin $allowed -or -not $seen.Add($entry.FullName) -or $entry.Length -le 0 -or $entry.Length -gt 160MB -or (($entry.ExternalAttributes -shr 16) -band 0xf000) -eq 0xa000) { throw 'Portable archive is incompatible with the legacy updater.' }
            $size += $entry.Length
            if ($size -gt 256MB) { throw 'Legacy update extraction limit exceeded.' }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry,(Join-Path $staged $entry.FullName))
        }
        foreach ($name in $runtimeFiles) { Assert ($seen.Contains($name)) "Legacy updater can read required file $name" }
    } finally { $zip.Dispose() }
    $stagedExe = Join-Path $staged 'Связь.exe'
    Assert ((Get-FileHash $candidate).Hash -eq (Get-FileHash $stagedExe).Hash) 'Portable archive contains exactly the candidate executable'
    Run-Checked $stagedExe @('--install')
    $next = Pointer current
    Assert ($next.Version -eq $version.ToString()) 'Rust maintenance registered its version'
    Move-Item "$update/current.json" "$update/pending.json"
    Install-Package $oldSetup legacy
    $old = Pointer current
    Assert ($old.Version -eq '0.13.3') 'The official 0.13.3 installer registered its own runtime'
    $knownExecutables += (Executable $next), (Executable $old)
    Assert-Links $old
    [IO.File]::WriteAllText($settings,$settingsSeed.Replace('"AutoUpdate":false','"AutoUpdate":true'))
    Start-Handoff $app
    Assert ((Pointer current).Id -eq $next.Id -and -not (Test-Path "$update/pending.json")) '0.13.3 promoted the native candidate after ready acknowledgement'
    Assert-Runtime $next $true $false
    Assert-Links $next
    # The first launch opens release notes. Capture the normal four-tab layout on the next launch.
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $prefs = Get-Content $preferences -Raw | ConvertFrom-Json -AsHashtable
        if ($prefs['LastSeenAppVersion'] -eq $version.ToString()) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    Assert ($prefs['LastSeenAppVersion'] -eq $version.ToString()) 'First launch recorded the release notes'
    Stop-ExactProcess (Executable $next)
    Start-Process -FilePath (Executable $next) -ArgumentList '--skip-self-update' -Environment @{ SVYAZ_TEST_FRAME_COUNTER = '1' }
    Assert-Runtime $next $true
    Stop-ExactProcess (Executable $next)
    $loaded = Get-Content $settings -Raw | ConvertFrom-Json
    Assert ($loaded.LastStrategy -eq 'migration-fixture' -and $loaded.Telegram.Port -eq 32412) 'Upgrade kept the selected strategy and Telegram port'
    $savedPreferences = Get-Content $preferences -Raw | ConvertFrom-Json
    Assert ($savedPreferences.NetworkStrategies.'migration-network' -eq 'migration-alt') 'Network profile survives the upgrade'
    Assert ($savedPreferences.QuietNotifications -eq $true) 'Upgrade keeps quiet notifications enabled'
    Assert-TrayStartup (Executable $next)

    # Seed a persisted older selection exactly as the previous updater would after user rollback.
    # Starting the actual Rust launcher now exercises Rust->WPF readiness, promotion and shortcut writes.
    [IO.File]::WriteAllText($settings,$settingsSeed)
    $oldJson = $old | ConvertTo-Json -Compress
    [IO.File]::WriteAllText("$update/rollback.json",$oldJson)
    [IO.File]::WriteAllText("$update/current.json",$oldJson)
    Start-Handoff (Executable $next)
    Assert ((Pointer current).Id -eq $old.Id -and (Pointer rollback).Id -eq $old.Id) 'Rust honored the persisted rollback'
    Assert-Runtime $old $false
    Assert-Links $old
    Stop-ExactProcess (Executable $old)

    Install-Package $nativeSetup native
    Assert ((Pointer current).Id -eq $next.Id) 'Native setup reused the verified native installation'
    Assert-Links $next
    $afterSettings = [IO.File]::ReadAllBytes($settings)
    $afterPreferences = [IO.File]::ReadAllBytes($preferences)
    $nativeExe = Executable $next
    $dll = Join-Path (Split-Path $nativeExe) 'wpfgfx_cor3.dll'
    $originalHash = (Get-FileHash -LiteralPath $dll).Hash
    Remove-Item -LiteralPath $dll
    Register-ScheduledTask -TaskName $ownTask -Action (New-ScheduledTaskAction -Execute (Executable $old) -Argument '--startup') -Trigger (New-ScheduledTaskTrigger -AtLogOn) -Force | Out-Null
    Disable-ScheduledTask -TaskName $ownTask | Out-Null
    Register-ScheduledTask -TaskName $foreignTask -Action (New-ScheduledTaskAction -Execute "$env:SystemRoot/System32/notepad.exe") -Force | Out-Null
    Install-Package $nativeSetup repair
    Assert ((Get-FileHash -LiteralPath $dll).Hash -eq $originalHash) 'Repair restored the genuine compatibility DLL'
    $task = Get-ScheduledTask -TaskName $ownTask
    Assert ($task.Actions.Execute -eq $nativeExe -and $task.Actions.Arguments -eq '--startup' -and $task.State -eq 'Disabled') 'Repair migrated the owned task while preserving arguments and disabled state'
    $unins = (Get-ChildItem -LiteralPath $root -Filter 'unins*.exe' | Select-Object -First 1).FullName
    Run-Checked $unins @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART')
    Assert (-not (Test-Path $app) -and -not (Test-Path "$root/Versions")) 'Native uninstall removed application files'
    Assert (-not ($links | Where-Object { Test-Path -LiteralPath $_ })) 'Native uninstall removed both owned shortcuts'
    Assert ($null -eq (Get-ScheduledTask -TaskName $ownTask -ErrorAction SilentlyContinue)) 'Native uninstall removed the owned task'
    Assert ($null -ne (Get-ScheduledTask -TaskName $foreignTask -ErrorAction SilentlyContinue)) 'Native uninstall preserved the foreign task'
    foreach ($pointerName in 'pending','current','rollback','rejected') { Assert (-not (Test-Path "$update/$pointerName.json")) "Native uninstall cleared $pointerName pointer" }
    Assert ([Convert]::ToBase64String([IO.File]::ReadAllBytes($settings)) -ceq [Convert]::ToBase64String($afterSettings)) 'Repair and uninstall preserved settings bytes'
    Assert ([Convert]::ToBase64String([IO.File]::ReadAllBytes($preferences)) -ceq [Convert]::ToBase64String($afterPreferences)) 'Repair and uninstall preserved preferences bytes'
}
finally {
    foreach ($log in 'startup-error.log','app.log') {
        $source = Join-Path $data $log
        if (Test-Path -LiteralPath $source) {
            Copy-Item -LiteralPath $source -Destination (Join-Path $PWD "dist/rust-$log")
        }
    }
    foreach ($exe in $knownExecutables) { Stop-ExactProcess $exe }
    if ($staged -and (Test-Path -LiteralPath $staged)) { Remove-Item -LiteralPath $staged -Recurse -Force }
    foreach ($taskName in $ownTask,$foreignTask) {
        if (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue) { Unregister-ScheduledTask -TaskName $taskName -Confirm:$false }
    }
}
