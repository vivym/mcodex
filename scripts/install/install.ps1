param(
    [Parameter(Position = 0)]
    [string]$Version = "latest"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Write-Step {
    param(
        [string]$Message
    )

    Write-Host "==> $Message"
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
        WrapperPath = Join-Path $wrapperDir "mcodex.ps1"
        DownloadBaseUrl = $downloadBaseUrl.TrimEnd("/")
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

function Resolve-RequestedVersion {
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

function Test-IsJunction {
    param(
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }

    $item = Get-Item -LiteralPath $Path -Force
    return $item.PSIsContainer -and (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
}

function Switch-CurrentJunction {
    param(
        [string]$BaseRoot,
        [string]$CurrentLink,
        [string]$TargetDir
    )

    if ((Test-Path -LiteralPath $CurrentLink) -and -not (Test-IsJunction -Path $CurrentLink)) {
        throw "$CurrentLink exists and is not a junction. Move it aside and rerun the installer."
    }

    $currentLeaf = Split-Path $CurrentLink -Leaf
    $tempLeaf = ".current.{0}.tmp" -f [System.Guid]::NewGuid().ToString("N")
    $backupLeaf = ".current.{0}.bak" -f [System.Guid]::NewGuid().ToString("N")
    $tempPath = Join-Path $BaseRoot $tempLeaf
    $backupPath = Join-Path $BaseRoot $backupLeaf

    New-Item -ItemType Junction -Path $tempPath -Target $TargetDir | Out-Null
    try {
        if (Test-Path -LiteralPath $CurrentLink) {
            Rename-Item -LiteralPath $CurrentLink -NewName $backupLeaf
            try {
                Rename-Item -LiteralPath $tempPath -NewName $currentLeaf
            } catch {
                if (Test-Path -LiteralPath $CurrentLink) {
                    Remove-Item -LiteralPath $CurrentLink -Recurse -Force
                }
                if (Test-Path -LiteralPath $backupPath) {
                    Rename-Item -LiteralPath $backupPath -NewName $currentLeaf
                }
                throw
            }

            Remove-Item -LiteralPath $backupPath -Recurse -Force
            return
        }

        Rename-Item -LiteralPath $tempPath -NewName $currentLeaf
    } catch {
        if (Test-Path -LiteralPath $tempPath) {
            Remove-Item -LiteralPath $tempPath -Recurse -Force
        }
        throw
    }
}

function Write-Wrapper {
    param(
        [string]$WrapperPath,
        [string]$InstalledBaseRoot
    )

    $baseRootLiteral = Convert-ToSingleQuotedLiteral -Value $InstalledBaseRoot
    $wrapper = @"
$BaseRoot = if (`$env:MCODEX_INSTALL_ROOT) { `$env:MCODEX_INSTALL_ROOT } else { $baseRootLiteral }
$Target = Join-Path `$BaseRoot "current\bin\mcodex.exe"
if (-not (Test-Path `$Target)) {
    Write-Error "mcodex installation missing or corrupted; rerun the installer."
    exit 1
}
`$env:MCODEX_INSTALL_MANAGED = "1"
`$env:MCODEX_INSTALL_METHOD = "script"
`$env:MCODEX_INSTALL_ROOT = `$BaseRoot
`$env:Path = "$(Join-Path `$BaseRoot "current\bin");`$env:Path"
& `$Target @args
exit `$LASTEXITCODE
"@

    Write-Utf8File -Path $WrapperPath -Content $wrapper
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
        [string]$WrapperDir
    )

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")

    if ($env:MCODEX_SKIP_USER_PATH_REGISTRY -eq "1") {
        if (-not (Path-Contains -PathValue $env:Path -Entry $WrapperDir)) {
            if ([string]::IsNullOrWhiteSpace($env:Path)) {
                $env:Path = $WrapperDir
            } else {
                $env:Path = "$WrapperDir;$env:Path"
            }
        }

        return [PSCustomObject]@{
            Action = "configured"
        }
    }

    if (-not (Path-Contains -PathValue $userPath -Entry $WrapperDir)) {
        if ([string]::IsNullOrWhiteSpace($userPath)) {
            $newUserPath = $WrapperDir
        } else {
            $newUserPath = "$WrapperDir;$userPath"
        }

        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
        if (-not (Path-Contains -PathValue $env:Path -Entry $WrapperDir)) {
            if ([string]::IsNullOrWhiteSpace($env:Path)) {
                $env:Path = $WrapperDir
            } else {
                $env:Path = "$WrapperDir;$env:Path"
            }
        }

        return [PSCustomObject]@{
            Action = "added"
        }
    }

    if (-not (Path-Contains -PathValue $env:Path -Entry $WrapperDir)) {
        if ([string]::IsNullOrWhiteSpace($env:Path)) {
            $env:Path = $WrapperDir
        } else {
            $env:Path = "$WrapperDir;$env:Path"
        }

        return [PSCustomObject]@{
            Action = "configured"
        }
    }

    return [PSCustomObject]@{
        Action = "already"
    }
}

function Invoke-McodexInstall {
    param(
        [string]$Version = "latest"
    )

    $platform = Get-PlatformDetails
    $config = Get-InstallConfig

    Write-Step "Installing mcodex CLI"
    Write-Step "Detected platform: $($platform.PlatformLabel)"

    New-Item -ItemType Directory -Force -Path $config.BaseRoot, $config.VersionsDir | Out-Null

    $resolvedVersion = Resolve-RequestedVersion -RequestedVersion $Version -DownloadBaseUrl $config.DownloadBaseUrl
    $tmpRoot = New-TemporaryDirectory -ParentPath $config.BaseRoot -Prefix ".install"
    $stagingDir = $null

    try {
        $checksumsPath = Join-Path $tmpRoot "SHA256SUMS"
        Download-File -Url "$($config.DownloadBaseUrl)/repositories/mcodex/releases/$resolvedVersion/SHA256SUMS" -OutFile $checksumsPath

        $expectedSha = Get-ExpectedSha256 -ChecksumsPath $checksumsPath -ArchiveName $platform.ArchiveName
        if ([string]::IsNullOrWhiteSpace($expectedSha)) {
            throw "No checksum entry found for $($platform.ArchiveName)."
        }

        $versionDir = Join-Path $config.VersionsDir $resolvedVersion
        Write-Step "Installing mcodex CLI $resolvedVersion"

        if (-not (Test-VersionDirectoryComplete -Directory $versionDir -Version $resolvedVersion -ArchiveName $platform.ArchiveName -Sha256 $expectedSha)) {
            $stagingDir = Stage-VersionDirectory -Config $config -Version $resolvedVersion -ArchiveName $platform.ArchiveName -ExpectedSha $expectedSha
            Publish-VersionDirectory -VersionDir $versionDir -StagingDir $stagingDir
            $stagingDir = $null
        }

        Switch-CurrentJunction -BaseRoot $config.BaseRoot -CurrentLink $config.CurrentLink -TargetDir $versionDir
        Write-Wrapper -WrapperPath $config.WrapperPath -InstalledBaseRoot $config.BaseRoot
        Write-InstallMetadata -MetadataFile $config.MetadataFile -Version $resolvedVersion -InstalledAt (Get-Timestamp) -BaseRoot $config.BaseRoot -VersionsDir $config.VersionsDir -CurrentLink $config.CurrentLink -WrapperPath $config.WrapperPath
        $pathAction = Add-WrapperDirToUserPath -WrapperDir $config.WrapperDir
    } finally {
        if ($stagingDir -and (Test-Path -LiteralPath $stagingDir)) {
            Remove-Item -LiteralPath $stagingDir -Recurse -Force -ErrorAction SilentlyContinue
        }
        if (Test-Path -LiteralPath $tmpRoot) {
            Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    switch ($pathAction.Action) {
        "added" {
            Write-Step "PATH updated for future PowerShell sessions."
        }
        "configured" {
            Write-Step "PATH is already configured for future PowerShell sessions."
        }
        default {
            Write-Step "$($config.WrapperDir) is already on PATH."
        }
    }

    Write-Host "mcodex CLI $resolvedVersion installed successfully."
}

if ($MyInvocation.InvocationName -ne ".") {
    Invoke-McodexInstall -Version $Version
}
