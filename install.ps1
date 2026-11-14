# Installs the latest `da` release on Windows.
#
#   irm https://raw.githubusercontent.com/LnYo-Cly/do-anything/main/install.ps1 | iex
#   irm https://cdn.jsdelivr.net/gh/LnYo-Cly/do-anything@main/install.ps1 | iex
#
# Environment:
#   DA_VERSION         release tag to install, for example v0.1.0 (default: latest)
#   DA_INSTALL_DIR     directory that receives da.exe (default: %LOCALAPPDATA%\Programs\da)
#   DA_DOWNLOAD_URL    base URL that holds the release files, for a mirror or proxy
#                      (default: https://github.com/LnYo-Cly/do-anything/releases/download)
#   DA_NO_MODIFY_PATH  set to any value to leave the user PATH unchanged

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# Windows PowerShell 5.1 does not enable TLS 1.2 by default.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Repo = 'LnYo-Cly/do-anything'

function Fail([string]$Message) {
    # `exit` would close the window when run through `irm | iex`; throw stops the script only.
    throw "da install: $Message"
}

function Get-WithRetry([string]$Uri, [string]$OutFile) {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing -TimeoutSec 60
            return
        } catch {
            if ($attempt -eq 3) { throw }
            Start-Sleep -Seconds 2
        }
    }
}

# The machine's architecture, not the process's: an x64 PowerShell emulated on
# Arm still gets the native Arm build.
try {
    $Arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
} catch {
    $Arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
}
$Targets = switch ($Arch) {
    { $_ -in 'X64', 'AMD64' } { @('x86_64-pc-windows-msvc') }
    # Prefer the native Arm build; the x86_64 one runs through emulation.
    { $_ -in 'Arm64', 'ARM64' } { @('aarch64-pc-windows-msvc', 'x86_64-pc-windows-msvc') }
    default { Fail "unsupported CPU $Arch; da is built for 64-bit x86 and Arm" }
}

$InstallDir = if ($env:DA_INSTALL_DIR) { $env:DA_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\da' }

$Version = $env:DA_VERSION
if (-not $Version) {
    # The /releases/latest page redirects to /releases/tag/<tag>; this avoids the rate-limited GitHub API.
    try {
        $request = [Net.WebRequest]::Create("https://github.com/$Repo/releases/latest")
        $request.Method = 'HEAD'
        $request.Timeout = 30000
        $response = $request.GetResponse()
        $latest = $response.ResponseUri.AbsoluteUri
        $response.Close()
    } catch {
        Fail "could not reach github.com to find the latest release; set DA_VERSION and DA_DOWNLOAD_URL to use a mirror"
    }
    $Version = $latest.Split('/')[-1]
    if (-not $Version.StartsWith('v')) {
        Fail "could not read the latest release tag from $latest; set DA_VERSION, for example `$env:DA_VERSION = 'v0.1.0'"
    }
}

$Base = if ($env:DA_DOWNLOAD_URL) { $env:DA_DOWNLOAD_URL.TrimEnd('/') } else { "https://github.com/$Repo/releases/download" }
$Base = "$Base/$Version"

$Temp = Join-Path ([IO.Path]::GetTempPath()) ("da-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Temp | Out-Null
try {
    try { Get-WithRetry "$Base/SHA256SUMS" (Join-Path $Temp 'SHA256SUMS') }
    catch { Fail "could not download $Base/SHA256SUMS; check that $Version exists" }
    $sums = @{}
    foreach ($line in Get-Content (Join-Path $Temp 'SHA256SUMS')) {
        $fields = $line.Trim() -split '\s+'
        if ($fields.Count -ge 2) { $sums[$fields[1].TrimStart('*')] = $fields[0].ToLowerInvariant() }
    }

    # The first target this release was built for.
    $Target = $Targets | Where-Object { $sums.ContainsKey("da-$Version-$_.zip") } | Select-Object -First 1
    if (-not $Target) { Fail "$Version has no Windows build for $Arch" }
    $Archive = "da-$Version-$Target.zip"

    Write-Host "Downloading da $Version for $Target"
    try { Get-WithRetry "$Base/$Archive" (Join-Path $Temp $Archive) }
    catch { Fail "could not download $Base/$Archive" }

    $expected = $sums[$Archive]
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $Temp $Archive)).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Fail "checksum mismatch for $Archive (expected $expected, got $actual); nothing was installed"
    }

    Expand-Archive -Path (Join-Path $Temp $Archive) -DestinationPath $Temp -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $exe = Join-Path $InstallDir 'da.exe'
    # Copy then rename, so an existing da.exe is never replaced by a half-written file.
    Copy-Item (Join-Path $Temp "da-$Version-$Target\da.exe") "$exe.new" -Force
    Move-Item "$exe.new" $exe -Force
} finally {
    Remove-Item -Recurse -Force $Temp -ErrorAction SilentlyContinue
}

Write-Host "Installed $(& $exe --version) to $exe"

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = if ($userPath) { $userPath.Split(';') } else { @() }
if ($env:DA_NO_MODIFY_PATH) {
    Write-Host "DA_NO_MODIFY_PATH is set; add $InstallDir to PATH yourself."
} elseif ($entries -notcontains $InstallDir) {
    $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Added $InstallDir to your user PATH. Open a new terminal to use da."
}
