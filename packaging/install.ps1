<#
.SYNOPSIS
    Installs snob on Windows.

.DESCRIPTION
    irm https://raw.githubusercontent.com/dennisgr7/snob/main/packaging/install.ps1 | iex

    Reads three optional environment variables:

        SNOB_VERSION      a version to install instead of the latest
        SNOB_INSTALL_DIR  where to put it; default %LOCALAPPDATA%\Programs\snob
        SNOB_EDITION      desktop (the app, with the command line and the MCP
                          server beside it) or cli (those two alone)

    Without SNOB_EDITION the desktop edition is installed where the release
    lists it for this machine, and the command line alone otherwise, said so.
    The app's two binaries go to the same directory: the app starts the
    snob.exe beside it, and that is also how an update knows the edition.
#>

$ErrorActionPreference = 'Stop'

$repo = 'dennisgr7/snob'
$installDir = if ($env:SNOB_INSTALL_DIR) { $env:SNOB_INSTALL_DIR }
              else { Join-Path $env:LOCALAPPDATA 'Programs\snob' }

# ARM64 Windows reports AMD64 to a 32-bit or emulated process, so the
# environment variable alone is not enough to tell the two apart.
$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    'Arm64' { 'aarch64-pc-windows-msvc' }
    'X64'   { 'x86_64-pc-windows-msvc' }
    default { throw "snob has no build for $_" }
}

# A leading v is how the tags are spelled and how people type a version; the
# archive names carry one of their own.
$version = $env:SNOB_VERSION -replace '^v', ''
if (-not $version) {
    $latest = Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$repo/releases/latest"
    $version = $latest.tag_name -replace '^v', ''
}

$base = "https://github.com/$repo/releases/download/v$version"
$work = Join-Path ([System.IO.Path]::GetTempPath()) "snob-install-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Force $work | Out-Null

try {
    $sumsFile = Join-Path $work 'SHA256SUMS'
    Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile $sumsFile
    $sums = Get-Content $sumsFile
    # The checksum SHA256SUMS lists for a file, or nothing.
    function Listed([string]$file) {
        foreach ($line in $sums) {
            $fields = $line -split '\s+'
            if ($fields.Count -ge 2 -and ($fields[1] -eq $file -or $fields[1] -eq "*$file")) {
                return $fields[0]
            }
        }
        return $null
    }

    # Which edition, from what the release lists for this machine.
    $desktop = "snob-desktop-v$version-$arch"
    $edition = $env:SNOB_EDITION
    switch ($edition) {
        'desktop' {
            if (-not (Listed "$desktop.zip")) {
                throw "snob $version has no desktop app for this machine; SNOB_EDITION=cli installs the command line"
            }
        }
        'cli' {}
        { -not $_ } {
            if (Listed "$desktop.zip") { $edition = 'desktop' }
            else {
                $edition = 'cli'
                Write-Host "snob $version has no desktop app for this machine: installing the command line and the MCP server"
            }
        }
        default { throw "SNOB_EDITION is desktop or cli, not $edition" }
    }
    if ($edition -eq 'desktop') {
        $name = $desktop
        Write-Host "Downloading snob $version, the desktop app, for $arch"
    } else {
        $name = "snob-v$version-$arch"
        Write-Host "Downloading snob $version for $arch"
    }
    $zip = Join-Path $work "$name.zip"
    Invoke-WebRequest -UseBasicParsing "$base/$name.zip" -OutFile $zip

    # Not optional: this downloads an executable and puts it on the PATH.
    $expected = Listed "$name.zip"
    if (-not $expected) { throw "$name.zip is not listed in SHA256SUMS" }
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "checksum mismatch: expected $expected, got $actual" }

    # The checksum says the file is the one the release page lists, not who
    # built it: SHA256SUMS comes from the same page as the archive. The release
    # workflow signs build provenance through Sigstore, and gh is the client
    # that reads it. Skipped, out loud, when gh is not here.
    #
    # A check that could not run is not a check that failed: `gh attestation`
    # arrived in 2.49 and needs a login, and dying with "does not carry a valid
    # build provenance" accused the release of tampering for something about
    # this machine.
    $canAttest = $false
    if (Get-Command gh -ErrorAction SilentlyContinue) {
        # Windows PowerShell 5.1 turns a native command's standard error into
        # a terminating error under 'Stop', redirected or not; these two are
        # questions whose "no" is an answer.
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            & gh attestation --help *> $null
            $hasCommand = $LASTEXITCODE -eq 0
            & gh auth status *> $null
            $canAttest = $hasCommand -and $LASTEXITCODE -eq 0
        } finally {
            $ErrorActionPreference = $previous
        }
    }
    if ($canAttest) {
        # The provenance keeps the name the repository had when it was
        # signed, which was `snob-cli` up to 0.9.0.
        $parts = @($version.Split('.') | ForEach-Object { [int]($_ -replace '\D.*', '') })
        $signer = if ($parts[0] -eq 0 -and $parts[1] -le 9) { 'dennisgr7/snob-cli' } else { $repo }
        & gh attestation verify $zip --repo $signer `
            --signer-workflow "$signer/.github/workflows/release.yml" | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "$name.zip does not carry a valid build provenance from $signer's release workflow"
        }
        Write-Host "Build provenance verified"
    } else {
        Write-Host "note: gh is missing, older than 2.49 or not logged in, so the build provenance was not verified (the checksum was)."
    }

    Expand-Archive $zip -DestinationPath $work -Force
    New-Item -ItemType Directory -Force $installDir | Out-Null

    # A running snob.exe -- `snob watch` as a scheduled service, most often --
    # or a running app cannot be overwritten or removed. Windows does let a
    # running executable be renamed, so one that will not go is moved aside,
    # and removed by the next install; one that is not running is simply
    # removed. A copy that fails puts the old ones back rather than leaving
    # no snob.exe at all.
    $exe = Join-Path $installDir 'snob.exe'
    $binaries = @('snob.exe')
    if ($edition -eq 'desktop') { $binaries = @('snob-desktop.exe') + $binaries }
    $asides = @{}
    foreach ($binary in $binaries) {
        $path = Join-Path $installDir $binary
        Get-ChildItem $installDir -Filter "$binary.old-*" -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        if (Test-Path $path) {
            Remove-Item $path -Force -ErrorAction SilentlyContinue
        }
        if (Test-Path $path) {
            $asides[$path] = Join-Path $installDir "$binary.old-$([System.Guid]::NewGuid())"
            Move-Item $path $asides[$path] -Force
        }
    }
    try {
        Copy-Item (Join-Path $work "$name\*") $installDir -Force
    } catch {
        foreach ($path in $asides.Keys) {
            if (-not (Test-Path $path)) { Move-Item $asides[$path] $path -Force }
        }
        throw
    }

    Write-Host "Installed $(& $exe --version) to $installDir"

    # The user's PATH, never the machine's: this installs for one account and
    # has no business editing anything the whole system reads.
    #
    # Read and written through the registry rather than through
    # [Environment]::GetEnvironmentVariable, which expands %VAR% references
    # before handing the value over. Writing that back turns a PATH holding
    # %USERPROFILE%\.cargo\bin into the literal expanded path, silently and
    # permanently, which breaks a roaming profile or a relocated home directory
    # for every tool that was relying on it.
    $onPath = $false
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $userPath = $key.GetValue(
            'Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        # An account that has never had a user PATH has no value to read, and
        # calling a method on the $null that comes back is a terminating error
        # under $ErrorActionPreference = 'Stop' -- after the binary has already
        # been copied, so the install fails having half worked.
        if ($null -eq $userPath) { $userPath = '' }

        # Split on the separator and compare whole entries. `-like` treats [ ]
        # as a character class, so a bracket in the path made the test
        # meaningless; and a substring match called it present when PATH merely
        # contained a longer path starting with this one.
        $entries = $userPath.Split(';') | Where-Object { $_ -ne '' }
        if ($entries -contains $installDir) {
            $onPath = $true
        }
        else {
            $updated = (@($entries) + $installDir) -join ';'
            # ExpandString, so any %VAR% left in the value keeps expanding.
            $key.SetValue('Path', $updated, [Microsoft.Win32.RegistryValueKind]::ExpandString)

            # Writing the value is not enough. Environment blocks are inherited
            # at process start, so a "new terminal" is spawned by the explorer.exe
            # that is already running with the old block and gets the old PATH.
            # Without this broadcast the message below is false until the user
            # logs out, which is exactly what it says is unnecessary.
            #
            # Best-effort on purpose: $ErrorActionPreference is 'Stop', Add-Type
            # compiles at run time and can legitimately fail, and by this point
            # the binary is already installed. A failed broadcast costs a stale
            # terminal; a failed install costs the install.
            $broadcast = $false
            try {
                if (-not ('SnobEnv' -as [type])) {
                    Add-Type -Namespace '' -Name 'SnobEnv' -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
    IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
                }
                $HWND_BROADCAST = [IntPtr]0xffff
                $WM_SETTINGCHANGE = 0x1a
                $SMTO_ABORTIFHUNG = 0x2
                $result = [UIntPtr]::Zero
                [void][SnobEnv]::SendMessageTimeout(
                    $HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, 'Environment',
                    $SMTO_ABORTIFHUNG, 5000, [ref]$result)
                $broadcast = $true
            }
            catch {
                $broadcast = $false
            }

            if ($broadcast) {
                Write-Host "Added $installDir to your PATH. Open a new terminal for it to take effect."
            }
            else {
                Write-Host "Added $installDir to your PATH. Sign out and back in for it to take effect."
            }
        }
    }
    finally {
        if ($key) { $key.Dispose() }
    }

    # The commands below have to be ones the user can actually type. Until the
    # new PATH reaches a fresh terminal, a bare `snob` does not resolve in this
    # one -- and telling somebody to run a command that fails is how the
    # `snob purge` reminder gets skipped, which leaves a live session cookie on
    # a machine whose owner has just uninstalled the tool.
    $snob = if ($onPath) { 'snob' } else { "`"$exe`"" }

    Write-Host ""
    if ($edition -eq 'desktop') {
        Write-Host "Open the app with: `"$(Join-Path $installDir 'snob-desktop.exe')`""
    }
    Write-Host "Start with: $snob login"
    Write-Host "Before uninstalling, run `"$snob purge`": the session and the database"
    Write-Host "live outside this directory and deleting the binary will not reach them."
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
