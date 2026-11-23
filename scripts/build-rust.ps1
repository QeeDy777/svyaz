#requires -Version 7.4
[CmdletBinding()]
param([string]$IsccPath = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe")
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Push-Location "$PSScriptRoot/.."
try {
    if (-not $IsWindows) { throw 'Build the Windows x64 package on Windows.' }
    $version = (Select-String -Path rust/app/Cargo.toml -Pattern '^version = "(\d+\.\d+\.\d+)[^"]*"$').Matches[0].Groups[1].Value
    $dist = Join-Path $PWD 'dist/rust-candidate'
    $cache = Join-Path $PWD 'dist/rust-compat'
    New-Item $cache -ItemType Directory -Force | Out-Null
    $archive = Join-Path $cache 'Svyaz-0.13.3-win-x64-unsigned.zip'
    $fixture = Join-Path $cache 'Svyaz-0.13.3-win-x64-setup-unsigned.exe'
    function LegacyFile($path, $hash) {
        if (-not (Test-Path -LiteralPath $path)) {
            $uri = "https://github.com/QeeDy777/svyaz/releases/download/v0.13.3/" + [IO.Path]::GetFileName($path)
            $tmp = "$path.$([guid]::NewGuid().ToString('N')).tmp"
            try {
                Invoke-WebRequest -Uri $uri -OutFile $tmp
                if ((Get-FileHash $tmp).Hash -ne $hash) { throw 'Invalid 0.13.3 fixture digest.' }
                Move-Item $tmp $path
            } finally { if (Test-Path $tmp) { Remove-Item $tmp } }
        }
        if ((Get-FileHash $path).Hash -ne $hash) { throw "Corrupt fixture: $path" }
    }
    LegacyFile $archive '4c473135561d6cfdebdabd21d41af611461939e2623e1cc61299e8779b4f97cd'
    LegacyFile $fixture 'd689478f324f48aa3095a48d97c8518f137784ef5070cdf2644abe6e3729ebbf'
    cargo build --locked --release -p svyaz --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Rust build failed' }
    $work = New-Item -ItemType Directory -Path "dist/rust-stage-$([guid]::NewGuid().ToString('N'))"
    try {
        $exe = Join-Path $work.FullName 'Связь.exe'
        Copy-Item target/x86_64-pc-windows-msvc/release/svyaz.exe $exe
        $info = (Get-Item -LiteralPath $exe).VersionInfo
        if ("$($info.FileMajorPart).$($info.FileMinorPart).$($info.FileBuildPart)" -ne $version) {throw 'Unexpected EXE version'}
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            # These are part of the immutable six-file identity checked by installed 0.13.3 launchers.
            foreach ($name in @('D3DCompiler_47_cor3.dll','PenImc_cor3.dll','PresentationNative_cor3.dll','vcruntime140_cor3.dll','wpfgfx_cor3.dll',
                    'DOTNET-LICENSE.txt','DOTNET-THIRD-PARTY-NOTICES.txt','WINDOWS-DESKTOP-LICENSE.txt','TG-WS-PROXY-LICENSE.txt')) {
                $entry = $zip.GetEntry($name)
                if ($null -eq $entry) {throw "Missing compatibility file $name"}
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $work.FullName $name))
            }
        } finally { $zip.Dispose() }
        foreach ($name in @('README','VALIDATION','SIGNING')) { Copy-Item "$name.md" (Join-Path $work.FullName "$name.txt") }
        if (Test-Path $dist) { Remove-Item -LiteralPath $dist -Recurse -Force }
        Move-Item -LiteralPath $work.FullName -Destination $dist
    } finally { if (Test-Path $work.FullName) {Remove-Item -LiteralPath $work.FullName -Recurse -Force} }
    $output = "dist/Svyaz-$version-win-x64-unsigned.zip"
    Compress-Archive "$dist/*" -DestinationPath $output -Force
    & $IsccPath "/DAppVersion=$version" "/DAppFiles=$dist" '/DVariant=unsigned' installer/Svyaz.iss
    if ($LASTEXITCODE) { throw 'Cannot compile native installer' }
    foreach ($path in @($output,"dist/Svyaz-$version-win-x64-setup-unsigned.exe")) {
        "$((Get-FileHash $path).Hash.ToLowerInvariant())  $([IO.Path]::GetFileName($path))" | Set-Content "$path.sha256" -Encoding utf8NoBOM
    }
} finally { Pop-Location }
