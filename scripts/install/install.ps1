param(
    [Alias("Release")]
    [Parameter(Position = 0)]
    [string]$Version = "latest"
)

function Write-Step {
    param(
        [string]$Message
    )

    Write-Host "==> $Message"
}

function Write-WarningStep {
    param(
        [string]$Message
    )

    Write-Warning $Message
}

function Write-Utf8File {
    param(
        [string]$Path,
        [string]$Content
    )

    $directory = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($directory)) {
        New-Item -ItemType Directory -Force -Path $directory | Out-Null
    }

    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Convert-ToSingleQuotedLiteral {
    param(
        [string]$Value
    )

    return "'{0}'" -f $Value.Replace("'", "''")
}

function Convert-ToCmdSetLiteral {
    param(
        [string]$Value
    )

    return ($Value -replace "\^", "^^") -replace "%", "%%"
}

function Get-Timestamp {
    return [System.DateTimeOffset]::UtcNow.ToString(
        "yyyy-MM-ddTHH:mm:ss'Z'",
        [System.Globalization.CultureInfo]::InvariantCulture
    )
}

function New-TemporaryDirectory {
    param(
        [string]$ParentPath,
        [string]$Prefix
    )

    do {
        $leafName = "{0}.{1}" -f $Prefix, [System.Guid]::NewGuid().ToString("N")
        $candidate = Join-Path $ParentPath $leafName
    } while (Test-Path -LiteralPath $candidate)

    New-Item -ItemType Directory -Path $candidate | Out-Null
    return $candidate
}

function Download-File {
    param(
        [string]$Url,
        [string]$OutFile
    )

    $params = @{
        Uri = $Url
        OutFile = $OutFile
    }

    if ((Get-Command Invoke-WebRequest).Parameters.ContainsKey("UseBasicParsing")) {
        $params.UseBasicParsing = $true
    }

    Invoke-WebRequest @params | Out-Null
}

function Get-InstallConfig {
    $localAppData = $env:LOCALAPPDATA
    if (([string]::IsNullOrWhiteSpace($env:MCODEX_INSTALL_ROOT) -or [string]::IsNullOrWhiteSpace($env:MCODEX_WRAPPER_DIR)) -and [string]::IsNullOrWhiteSpace($localAppData)) {
        throw "LOCALAPPDATA is required to install mcodex."
    }

    $baseRoot = if (-not [string]::IsNullOrWhiteSpace($env:MCODEX_INSTALL_ROOT)) {
        $env:MCODEX_INSTALL_ROOT
    } else {
        Join-Path $localAppData "Mcodex"
    }

    $wrapperDir = if (-not [string]::IsNullOrWhiteSpace($env:MCODEX_WRAPPER_DIR)) {
        $env:MCODEX_WRAPPER_DIR
    } else {
        Join-Path $localAppData "Programs\Mcodex\bin"
    }

    $downloadBaseUrl = if (-not [string]::IsNullOrWhiteSpace($env:MCODEX_DOWNLOAD_BASE_URL)) {
        $env:MCODEX_DOWNLOAD_BASE_URL
    } else {
        "https://downloads.mcodex.sota.wiki"
    }

    return [PSCustomObject]@{
        BaseRoot = $baseRoot
        VersionsDir = Join-Path $baseRoot "install"
        CurrentLink = Join-Path $baseRoot "current"
        MetadataFile = Join-Path $baseRoot "install.json"
        WrapperDir = $wrapperDir
        WrapperPath = Join-Path $wrapperDir "mcodex.cmd"
        DownloadBaseUrl = $downloadBaseUrl.TrimEnd("/")
        LockPath = Join-Path $baseRoot "install.lock"
    }
}

function Normalize-Version {
    param(
        [string]$RawVersion
    )

    if ([string]::IsNullOrWhiteSpace($RawVersion) -or $RawVersion -eq "latest") {
        return "latest"
    }

    if ($RawVersion.StartsWith("rust-v")) {
        return $RawVersion.Substring(6)
    }

    if ($RawVersion.StartsWith("v")) {
        return $RawVersion.Substring(1)
    }

    return $RawVersion
}

function Assert-ValidVersion {
    param(
        [string]$NormalizedVersion,
        [string]$RequestedVersion
    )

    if ($NormalizedVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-((alpha|beta)\.[0-9]+))?$') {
        throw "Invalid version: $RequestedVersion"
    }
}

function Resolve-LatestVersion {
    param(
        [string]$DownloadBaseUrl
    )

    $manifest = Invoke-RestMethod -Uri "$DownloadBaseUrl/repositories/mcodex/channels/stable/latest.json"
    if ([string]::IsNullOrWhiteSpace($manifest.version)) {
        throw "Failed to resolve the latest mcodex release version."
    }

    Assert-ValidVersion -NormalizedVersion $manifest.version -RequestedVersion $manifest.version
    return $manifest.version
}

function Resolve-Version {
    param(
        [string]$RequestedVersion,
        [string]$DownloadBaseUrl
    )

    $normalizedVersion = Normalize-Version -RawVersion $RequestedVersion
    if ([string]::IsNullOrWhiteSpace($RequestedVersion) -or $RequestedVersion -eq "latest") {
        return Resolve-LatestVersion -DownloadBaseUrl $DownloadBaseUrl
    }

    Assert-ValidVersion -NormalizedVersion $normalizedVersion -RequestedVersion $RequestedVersion
    return $normalizedVersion
}

function Resolve-RequestedVersion {
    param(
        [string]$RequestedVersion,
        [string]$DownloadBaseUrl
    )

    return Resolve-Version -RequestedVersion $RequestedVersion -DownloadBaseUrl $DownloadBaseUrl
}

function Path-Contains {
    param(
        [string]$PathValue,
        [string]$Entry
    )

    if ([string]::IsNullOrWhiteSpace($PathValue) -or [string]::IsNullOrWhiteSpace($Entry)) {
        return $false
    }

    $needle = $Entry.TrimEnd("\")
    foreach ($segment in $PathValue.Split(";", [System.StringSplitOptions]::RemoveEmptyEntries)) {
        if ($segment.TrimEnd("\") -ieq $needle) {
            return $true
        }
    }

    return $false
}

function Get-WrapperDirPathUpdate {
    param(
        [string]$WrapperDir,
        [string]$UserPath,
        [string]$ProcessPath
    )

    if (-not (Path-Contains -PathValue $UserPath -Entry $WrapperDir)) {
        $nextUserPath = if ([string]::IsNullOrWhiteSpace($UserPath)) {
            $WrapperDir
        } else {
            "$WrapperDir;$UserPath"
        }

        $nextProcessPath = if (Path-Contains -PathValue $ProcessPath -Entry $WrapperDir) {
            $ProcessPath
        } elseif ([string]::IsNullOrWhiteSpace($ProcessPath)) {
            $WrapperDir
        } else {
            "$WrapperDir;$ProcessPath"
        }

        return [PSCustomObject]@{
            Action = "added"
            UserPath = $nextUserPath
            ProcessPath = $nextProcessPath
            UpdateUserPath = $true
        }
    }

    if (-not (Path-Contains -PathValue $ProcessPath -Entry $WrapperDir)) {
        $nextProcessPath = if ([string]::IsNullOrWhiteSpace($ProcessPath)) {
            $WrapperDir
        } else {
            "$WrapperDir;$ProcessPath"
        }

        return [PSCustomObject]@{
            Action = "configured"
            UserPath = $UserPath
            ProcessPath = $nextProcessPath
            UpdateUserPath = $false
        }
    }

    return [PSCustomObject]@{
        Action = "already"
        UserPath = $UserPath
        ProcessPath = $ProcessPath
        UpdateUserPath = $false
    }
}

function Get-PlatformDetails {
    if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)) {
        throw "install.ps1 supports Windows only. Use install.sh on macOS or Linux."
    }

    if (-not [System.Environment]::Is64BitOperatingSystem) {
        throw "mcodex requires a 64-bit version of Windows."
    }

    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    switch ($architecture) {
        "Arm64" {
            return [PSCustomObject]@{
                ArchiveName = "mcodex-win32-arm64.zip"
                PlatformLabel = "Windows (ARM64)"
            }
        }
        "X64" {
            return [PSCustomObject]@{
                ArchiveName = "mcodex-win32-x64.zip"
                PlatformLabel = "Windows (x64)"
            }
        }
        default {
            throw "Unsupported architecture: $architecture"
        }
    }
}

function Get-ExpectedSha256 {
    param(
        [string]$ChecksumsPath,
        [string]$ArchiveName
    )

    foreach ($line in [System.IO.File]::ReadAllLines($ChecksumsPath)) {
        if ($line -match '^\s*([0-9A-Fa-f]{64})\s+[*]?(.+?)\s*$' -and $Matches[2] -eq $ArchiveName) {
            return $Matches[1].ToUpperInvariant()
        }
    }

    return $null
}

function Write-JsonDocument {
    param(
        [string]$Path,
        [System.Collections.Specialized.OrderedDictionary]$Document
    )

    $json = $Document | ConvertTo-Json -Depth 5
    Write-Utf8File -Path $Path -Content ($json + [Environment]::NewLine)
}

function Write-CompletionMarker {
    param(
        [string]$Directory,
        [string]$Version,
        [string]$ArchiveName,
        [string]$Sha256,
        [string]$InstalledAt
    )

    $marker = [ordered]@{
        version = $Version
        archiveName = $ArchiveName
        sha256 = $Sha256
        installedAt = $InstalledAt
    }
    Write-JsonDocument -Path (Join-Path $Directory ".mcodex-install-complete.json") -Document $marker
}

function Test-VersionDirectoryComplete {
    param(
        [string]$Directory,
        [string]$Version,
        [string]$ArchiveName,
        [string]$Sha256
    )

    $markerPath = Join-Path $Directory ".mcodex-install-complete.json"
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        return $false
    }

    if (-not (Test-Path -LiteralPath (Join-Path $Directory "bin\mcodex.exe") -PathType Leaf)) {
        return $false
    }

    if (-not (Test-Path -LiteralPath (Join-Path $Directory "bin\rg.exe") -PathType Leaf)) {
        return $false
    }

    try {
        $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    } catch {
        return $false
    }

    return $marker.version -eq $Version -and $marker.archiveName -eq $ArchiveName -and $marker.sha256 -eq $Sha256
}

function Stage-VersionDirectory {
    param(
        [pscustomobject]$Config,
        [string]$Version,
        [string]$ArchiveName,
        [string]$ExpectedSha
    )

    $stagingDir = New-TemporaryDirectory -ParentPath $Config.VersionsDir -Prefix ".staging.$Version"
    $archivePath = Join-Path $stagingDir $ArchiveName
    $archiveUrl = "$($Config.DownloadBaseUrl)/repositories/mcodex/releases/$Version/$ArchiveName"

    Download-File -Url $archiveUrl -OutFile $archivePath
    $actualSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToUpperInvariant()
    if ($actualSha -ne $ExpectedSha) {
        throw "Archive checksum mismatch for $ArchiveName."
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $stagingDir -Force
    Remove-Item -LiteralPath $archivePath -Force

    if (-not (Test-Path -LiteralPath (Join-Path $stagingDir "bin\mcodex.exe") -PathType Leaf)) {
        throw "Archive layout for $ArchiveName is invalid."
    }

    if (-not (Test-Path -LiteralPath (Join-Path $stagingDir "bin\rg.exe") -PathType Leaf)) {
        throw "Archive layout for $ArchiveName is invalid."
    }

    Write-CompletionMarker -Directory $stagingDir -Version $Version -ArchiveName $ArchiveName -Sha256 $ExpectedSha -InstalledAt (Get-Timestamp)
    return $stagingDir
}

function Publish-VersionDirectory {
    param(
        [string]$VersionDir,
        [string]$StagingDir
    )

    if (-not (Test-Path -LiteralPath $VersionDir)) {
        Rename-Item -LiteralPath $StagingDir -NewName (Split-Path $VersionDir -Leaf)
        return
    }

    $parent = Split-Path -Parent $VersionDir
    $backupLeaf = ".replace.{0}.old" -f [System.Guid]::NewGuid().ToString("N")
    $backupPath = Join-Path $parent $backupLeaf

    Rename-Item -LiteralPath $VersionDir -NewName $backupLeaf
    try {
        if ($env:MCODEX_TEST_FAIL_AFTER_BACKUP -eq "1") {
            throw "Test failure after backing up existing version directory."
        }
        Rename-Item -LiteralPath $StagingDir -NewName (Split-Path $VersionDir -Leaf)
    } catch {
        if (Test-Path -LiteralPath $VersionDir) {
            Remove-Item -LiteralPath $VersionDir -Recurse -Force
        }
        if (Test-Path -LiteralPath $backupPath) {
            Rename-Item -LiteralPath $backupPath -NewName (Split-Path $VersionDir -Leaf)
        }
        throw
    }

    Remove-Item -LiteralPath $backupPath -Recurse -Force
}

function Add-JunctionSupportType {
    if (([System.Management.Automation.PSTypeName]'McodexInstaller.Junction').Type) {
        return
    }

    Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace McodexInstaller
{
    public static class Junction
    {
        private const uint GENERIC_WRITE = 0x40000000;
        private const uint FILE_SHARE_READ = 0x00000001;
        private const uint FILE_SHARE_WRITE = 0x00000002;
        private const uint FILE_SHARE_DELETE = 0x00000004;
        private const uint OPEN_EXISTING = 3;
        private const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
        private const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
        private const uint FSCTL_SET_REPARSE_POINT = 0x000900A4;
        private const uint IO_REPARSE_TAG_MOUNT_POINT = 0xA0000003;
        private const int HeaderLength = 20;

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern SafeFileHandle CreateFileW(
            string lpFileName,
            uint dwDesiredAccess,
            uint dwShareMode,
            IntPtr lpSecurityAttributes,
            uint dwCreationDisposition,
            uint dwFlagsAndAttributes,
            IntPtr hTemplateFile);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool DeviceIoControl(
            SafeFileHandle hDevice,
            uint dwIoControlCode,
            byte[] lpInBuffer,
            int nInBufferSize,
            IntPtr lpOutBuffer,
            int nOutBufferSize,
            out int lpBytesReturned,
            IntPtr lpOverlapped);

        public static void SetTarget(string linkPath, string targetPath)
        {
            string substituteName = "\\??\\" + Path.GetFullPath(targetPath);
            byte[] substituteNameBytes = Encoding.Unicode.GetBytes(substituteName);
            if (substituteNameBytes.Length > ushort.MaxValue - HeaderLength) {
                throw new ArgumentException("Junction target path is too long.", "targetPath");
            }

            byte[] reparseBuffer = new byte[substituteNameBytes.Length + HeaderLength];
            WriteUInt32(reparseBuffer, 0, IO_REPARSE_TAG_MOUNT_POINT);
            WriteUInt16(reparseBuffer, 4, checked((ushort)(substituteNameBytes.Length + 12)));
            WriteUInt16(reparseBuffer, 8, 0);
            WriteUInt16(reparseBuffer, 10, checked((ushort)substituteNameBytes.Length));
            WriteUInt16(reparseBuffer, 12, checked((ushort)(substituteNameBytes.Length + 2)));
            WriteUInt16(reparseBuffer, 14, 0);
            Buffer.BlockCopy(substituteNameBytes, 0, reparseBuffer, 16, substituteNameBytes.Length);

            using (SafeFileHandle handle = CreateFileW(
                linkPath,
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                IntPtr.Zero,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                IntPtr.Zero))
            {
                if (handle.IsInvalid) {
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                }

                int bytesReturned;
                if (!DeviceIoControl(
                    handle,
                    FSCTL_SET_REPARSE_POINT,
                    reparseBuffer,
                    reparseBuffer.Length,
                    IntPtr.Zero,
                    0,
                    out bytesReturned,
                    IntPtr.Zero))
                {
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                }
            }
        }

        private static void WriteUInt16(byte[] buffer, int offset, ushort value)
        {
            buffer[offset] = (byte)value;
            buffer[offset + 1] = (byte)(value >> 8);
        }

        private static void WriteUInt32(byte[] buffer, int offset, uint value)
        {
            buffer[offset] = (byte)value;
            buffer[offset + 1] = (byte)(value >> 8);
            buffer[offset + 2] = (byte)(value >> 16);
            buffer[offset + 3] = (byte)(value >> 24);
        }
    }
}
"@
}

function Set-JunctionTarget {
    param(
        [string]$LinkPath,
        [string]$TargetPath
    )

    Add-JunctionSupportType
    [McodexInstaller.Junction]::SetTarget($LinkPath, $TargetPath)
}

function Test-IsJunction {
    param(
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }

    $item = Get-Item -LiteralPath $Path -Force
    return ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -and $item.LinkType -eq "Junction"
}

function Ensure-Junction {
    param(
        [string]$LinkPath,
        [string]$TargetPath,
        [string]$InstallerOwnedTargetPrefix
    )

    if (-not (Test-Path -LiteralPath $LinkPath)) {
        New-Item -ItemType Junction -Path $LinkPath -Target $TargetPath | Out-Null
        return
    }

    $item = Get-Item -LiteralPath $LinkPath -Force
    if (Test-IsJunction -Path $LinkPath) {
        $existingTarget = [string]$item.Target
        if (-not [string]::IsNullOrWhiteSpace($InstallerOwnedTargetPrefix)) {
            $ownedTargetPrefix = $InstallerOwnedTargetPrefix.TrimEnd("\")
            if (-not $existingTarget.StartsWith($ownedTargetPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Refusing to retarget junction at $LinkPath because it is not managed by this installer."
            }
        }
        if ($existingTarget.Equals($TargetPath, [System.StringComparison]::OrdinalIgnoreCase)) {
            return
        }

        Set-JunctionTarget -LinkPath $LinkPath -TargetPath $TargetPath
        return
    }

    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Refusing to replace non-junction reparse point at $LinkPath."
    }

    if ($item.PSIsContainer) {
        if ((Get-ChildItem -LiteralPath $LinkPath -Force | Select-Object -First 1) -ne $null) {
            throw "Refusing to replace non-empty directory at $LinkPath with a junction."
        }

        Remove-Item -LiteralPath $LinkPath -Force
        New-Item -ItemType Junction -Path $LinkPath -Target $TargetPath | Out-Null
        return
    }

    throw "Refusing to replace file at $LinkPath with a junction."
}

function Write-Wrapper {
    param(
        [string]$BaseRoot,
        [string]$WrapperPath
    )

    $baseRootLiteral = Convert-ToCmdSetLiteral -Value $BaseRoot
    $wrapper = @'
@echo off
setlocal
set "BaseRoot=%MCODEX_INSTALL_ROOT%"
if not defined BaseRoot set "BaseRoot=__BASE_ROOT_LITERAL__"
set "Target=%BaseRoot%\current\bin\mcodex.exe"
if not exist "%Target%" (
    echo mcodex installation missing or corrupted; rerun the installer. 1>&2
    exit /b 1
)
set "MCODEX_INSTALL_MANAGED=1"
set "MCODEX_INSTALL_METHOD=script"
set "MCODEX_INSTALL_ROOT=%BaseRoot%"
set "PATH=%BaseRoot%\current\bin;%PATH%"
"%Target%" %*
exit /b %ERRORLEVEL%
'@.Replace("__BASE_ROOT_LITERAL__", $baseRootLiteral)

    Write-Utf8File -Path $WrapperPath -Content $wrapper

    $legacyWrapperPath = [System.IO.Path]::ChangeExtension($WrapperPath, ".ps1")
    if (Test-Path -LiteralPath $legacyWrapperPath -PathType Leaf) {
        $legacyWrapper = Get-Content -LiteralPath $legacyWrapperPath -Raw
        if ($legacyWrapper.Contains("MCODEX_INSTALL_MANAGED") -and $legacyWrapper.Contains("current\bin\mcodex.exe")) {
            Remove-Item -LiteralPath $legacyWrapperPath -Force
        }
    }
}

function Write-InstallMetadata {
    param(
        [string]$MetadataFile,
        [string]$Version,
        [string]$InstalledAt,
        [string]$BaseRoot,
        [string]$VersionsDir,
        [string]$CurrentLink,
        [string]$WrapperPath
    )

    $metadata = [ordered]@{
        product = "mcodex"
        installMethod = "script"
        currentVersion = $Version
        installedAt = $InstalledAt
        baseRoot = $BaseRoot
        versionsDir = $VersionsDir
        currentLink = $CurrentLink
        wrapperPath = $WrapperPath
    }
    Write-JsonDocument -Path $MetadataFile -Document $metadata
}

function Add-WrapperDirToUserPath {
    param(
        [string]$WrapperDir,
        [scriptblock]$GetUserPath = {
            [Environment]::GetEnvironmentVariable("Path", "User")
        },
        [scriptblock]$SetUserPath = {
            param($PathValue)
            [Environment]::SetEnvironmentVariable("Path", $PathValue, "User")
        },
        [scriptblock]$SetProcessPath = {
            param($PathValue)
            $env:Path = $PathValue
        }
    )

    $userPath = & $GetUserPath

    $pathUpdate = Get-WrapperDirPathUpdate -WrapperDir $WrapperDir -UserPath $userPath -ProcessPath $env:Path
    if ($env:MCODEX_SKIP_USER_PATH_REGISTRY -ne "1" -and $pathUpdate.UpdateUserPath) {
        & $SetUserPath $pathUpdate.UserPath
    }

    if ($pathUpdate.ProcessPath -ne $env:Path) {
        & $SetProcessPath $pathUpdate.ProcessPath
    }

    return $pathUpdate
}

function Invoke-WithInstallLock {
    param(
        [string]$LockPath,
        [scriptblock]$Script
    )

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LockPath) | Out-Null
    $lock = $null
    while ($null -eq $lock) {
        try {
            $lock = [System.IO.File]::Open(
                $LockPath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
        } catch [System.IO.IOException] {
            Start-Sleep -Milliseconds 250
        }
    }

    try {
        & $Script
    } finally {
        $lock.Dispose()
    }
}

function Remove-StaleInstallArtifacts {
    param(
        [string]$VersionsDir,
        [string]$BaseRoot
    )

    if (Test-Path -LiteralPath $VersionsDir -PathType Container) {
        Get-ChildItem -LiteralPath $VersionsDir -Force -Directory -Filter ".staging.*" -ErrorAction SilentlyContinue |
            Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
        Get-ChildItem -LiteralPath $VersionsDir -Force -Directory -Filter ".replace.*.old" -ErrorAction SilentlyContinue |
            Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    }

    if (Test-Path -LiteralPath $BaseRoot -PathType Container) {
        Get-ChildItem -LiteralPath $BaseRoot -Force -Directory -Filter ".current.*" -ErrorAction SilentlyContinue |
            Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Get-VersionFromBinary {
    param(
        [string]$BinaryPath
    )

    if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
        return $null
    }

    try {
        $versionOutput = & $BinaryPath --version 2>$null
    } catch {
        return $null
    }

    if ($versionOutput -match '([0-9][0-9A-Za-z.+-]*)$') {
        return $Matches[1]
    }

    return $null
}

function Get-CurrentInstalledVersion {
    param(
        [string]$CurrentLink
    )

    return Get-VersionFromBinary -BinaryPath (Join-Path $CurrentLink "bin\mcodex.exe")
}

function Get-ExistingMcodexCommand {
    $existing = Get-Command mcodex -ErrorAction SilentlyContinue
    if ($null -eq $existing) {
        return $null
    }

    return $existing.Source
}

function Get-ConflictingInstall {
    param(
        [string]$WrapperPath,
        [string]$CurrentBinaryPath
    )

    $existingPath = Get-ExistingMcodexCommand
    if ([string]::IsNullOrWhiteSpace($existingPath)) {
        return $null
    }

    if ($existingPath.Equals($WrapperPath, [System.StringComparison]::OrdinalIgnoreCase) -or
        $existingPath.Equals($CurrentBinaryPath, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $null
    }

    Write-Step "Detected existing mcodex command at $existingPath"
    Write-WarningStep "Multiple mcodex installs can be ambiguous because PATH order decides which one runs."

    return [PSCustomObject]@{
        Path = $existingPath
    }
}

function Test-InstalledBinary {
    param(
        [string]$BinaryPath
    )

    if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
        throw "Installed mcodex binary is missing: $BinaryPath"
    }

    & $BinaryPath --version *> $null
    if ($LASTEXITCODE -ne 0) {
        throw "Installed mcodex command failed verification: $BinaryPath --version"
    }
}

function Print-LaunchInstructions {
    param(
        [string]$PathAction,
        [string]$WrapperDir
    )

    switch ($PathAction) {
        "added" {
            Write-Step "Current PowerShell session: `$env:Path = `"$WrapperDir;`$env:Path`"; mcodex"
            Write-Step "Future PowerShell windows: open a new PowerShell window and run: mcodex"
            Write-Step "PATH was added to the user environment."
        }
        "configured" {
            Write-Step "Current PowerShell session: `$env:Path = `"$WrapperDir;`$env:Path`"; mcodex"
            Write-Step "Future PowerShell windows: open a new PowerShell window and run: mcodex"
            Write-Step "PATH is already configured in the user environment."
        }
        default {
            Write-Step "Current PowerShell session: mcodex"
            Write-Step "Future PowerShell windows: open a new PowerShell window and run: mcodex"
        }
    }
}

function Invoke-McodexInstall {
    param(
        [string]$Version = "latest"
    )

    $originalErrorActionPreference = $ErrorActionPreference
    $originalProgressPreference = $ProgressPreference
    try {
        Set-StrictMode -Version Latest
        $ErrorActionPreference = "Stop"
        $ProgressPreference = "SilentlyContinue"

        $platform = Get-PlatformDetails
        $config = Get-InstallConfig
        $resolvedVersion = Resolve-Version -RequestedVersion $Version -DownloadBaseUrl $config.DownloadBaseUrl
        $currentVersion = Get-CurrentInstalledVersion -CurrentLink $config.CurrentLink

        if (-not [string]::IsNullOrWhiteSpace($currentVersion) -and $currentVersion -ne $resolvedVersion) {
            Write-Step "Updating mcodex CLI from $currentVersion to $resolvedVersion"
        } elseif (-not [string]::IsNullOrWhiteSpace($currentVersion)) {
            Write-Step "Updating mcodex CLI"
        } else {
            Write-Step "Installing mcodex CLI"
        }
        Write-Step "Detected platform: $($platform.PlatformLabel)"
        Write-Step "Resolved version: $resolvedVersion"

        New-Item -ItemType Directory -Force -Path $config.BaseRoot, $config.VersionsDir | Out-Null

        $conflictingInstall = Get-ConflictingInstall `
            -WrapperPath $config.WrapperPath `
            -CurrentBinaryPath (Join-Path $config.CurrentLink "bin\mcodex.exe")
        $tmpRoot = New-TemporaryDirectory -ParentPath $config.BaseRoot -Prefix ".install"
        $stagingDir = $null

        try {
            Invoke-WithInstallLock -LockPath $config.LockPath -Script {
                Remove-StaleInstallArtifacts -VersionsDir $config.VersionsDir -BaseRoot $config.BaseRoot

                $checksumsPath = Join-Path $tmpRoot "SHA256SUMS"
                Download-File -Url "$($config.DownloadBaseUrl)/repositories/mcodex/releases/$resolvedVersion/SHA256SUMS" -OutFile $checksumsPath

                $expectedSha = Get-ExpectedSha256 -ChecksumsPath $checksumsPath -ArchiveName $platform.ArchiveName
                if ([string]::IsNullOrWhiteSpace($expectedSha)) {
                    throw "No checksum entry found for $($platform.ArchiveName)."
                }

                $versionDir = Join-Path $config.VersionsDir $resolvedVersion
                if (-not (Test-VersionDirectoryComplete -Directory $versionDir -Version $resolvedVersion -ArchiveName $platform.ArchiveName -Sha256 $expectedSha)) {
                    if (Test-Path -LiteralPath $versionDir) {
                        Write-WarningStep "Found incomplete existing release at $versionDir. Reinstalling."
                    }

                    $stagingDir = Stage-VersionDirectory -Config $config -Version $resolvedVersion -ArchiveName $platform.ArchiveName -ExpectedSha $expectedSha
                    Publish-VersionDirectory -VersionDir $versionDir -StagingDir $stagingDir
                    $stagingDir = $null
                }

                Ensure-Junction -LinkPath $config.CurrentLink -TargetPath $versionDir -InstallerOwnedTargetPrefix $config.VersionsDir
                Write-Wrapper -BaseRoot $config.BaseRoot -WrapperPath $config.WrapperPath
                Write-InstallMetadata -MetadataFile $config.MetadataFile -Version $resolvedVersion -InstalledAt (Get-Timestamp) -BaseRoot $config.BaseRoot -VersionsDir $config.VersionsDir -CurrentLink $config.CurrentLink -WrapperPath $config.WrapperPath
                Test-InstalledBinary -BinaryPath (Join-Path $config.CurrentLink "bin\mcodex.exe")
            }

            $pathAction = Add-WrapperDirToUserPath -WrapperDir $config.WrapperDir
        } finally {
            if ($stagingDir -and (Test-Path -LiteralPath $stagingDir)) {
                Remove-Item -LiteralPath $stagingDir -Recurse -Force -ErrorAction SilentlyContinue
            }
            if (Test-Path -LiteralPath $tmpRoot) {
                Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
            }
        }

        if ($null -ne $conflictingInstall) {
            Write-WarningStep "Leaving the existing mcodex command installed at $($conflictingInstall.Path)."
        }

        switch ($pathAction.Action) {
            "added" {
                Print-LaunchInstructions -PathAction "added" -WrapperDir $config.WrapperDir
            }
            "configured" {
                Print-LaunchInstructions -PathAction "configured" -WrapperDir $config.WrapperDir
            }
            default {
                Write-Step "$($config.WrapperDir) is already on PATH."
                Print-LaunchInstructions -PathAction "already" -WrapperDir $config.WrapperDir
            }
        }

        Write-Host "mcodex CLI $resolvedVersion installed successfully."
    } finally {
        $ProgressPreference = $originalProgressPreference
        $ErrorActionPreference = $originalErrorActionPreference
    }
}

if ($MyInvocation.InvocationName -ne ".") {
    Invoke-McodexInstall -Version $Version
}
