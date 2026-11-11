# Helper the Wasl NSIS installer and uninstaller run (installer.nsi). Windows PowerShell 5.1, no modules.
#
#   wasl-installer.ps1 path-add          -Dir <install dir> [-Scope User|Machine]
#   wasl-installer.ps1 path-remove       -Dir <install dir> [-Scope User|Machine]
#   wasl-installer.ps1 run-entry-remove  -Dir <install dir>
#   wasl-installer.ps1 component-state   -Dir <install dir> -Link <desktop .lnk>
#   wasl-installer.ps1 stop-in-dir       -Dir <install dir> [-MsiProduct <{GUID}> [-Quiet]] [-DryRun]
#   wasl-installer.ps1 is-desktop-user
#
# path-add / path-remove edit the user's (or, for an all-users install, the machine's) PATH in the
# registry and keep its REG_EXPAND_SZ type and every other entry as it was; the installer then
# broadcasts WM_SETTINGCHANGE so a new terminal sees the change without a restart. path-add exits 10
# when the folder was already on PATH, so the installer knows the entry is not its own to remove.
# run-entry-remove deletes the "Start at sign-in" value `wasl service install` writes, only when it
# starts this install's wasld, so uninstalling one copy never turns off another copy's autostart.
# component-state tells an upgrade what an earlier install left: 1 the desktop shortcut points at this
# install, 2 the sign-in entry does.
# stop-in-dir stops wasld and the tray app only when they run from this install folder and belong to
# the person at the desktop, never a Wasl started from somewhere else or by another account. Its exit
# code says what was running: 1 wasld, 2 the app, 3 both. With -MsiProduct it then uninstalls that
# old .msi (Tauri's WiX build), which may share the folder; 8 is added when that fails.
# is-desktop-user exits 0 when this process runs as the person signed in at the desktop, 1 when it
# runs as someone else (an administrator who elevated the installer "over the shoulder").
#
# The sign-in entry and the processes are the desktop user's even when an administrator runs this.
# -EnvKey, -RunKey, -MsiExec and -DesktopSid point the commands elsewhere; the tests use them.

param(
    [Parameter(Position = 0)] [string] $Command,
    [string] $Dir,
    [ValidateSet('User', 'Machine')] [string] $Scope = 'User',
    [string] $EnvKey,
    [string] $RunKey,
    [string] $Link,
    [string] $MsiProduct,
    [string] $MsiExec = "$env:SystemRoot\System32\msiexec.exe",
    [switch] $Quiet,
    [switch] $DryRun,
    [string] $DesktopSid
)

$ErrorActionPreference = 'Stop'
# A failure exits 70, never 1-3 or 8, which the installer reads as answers.
trap { [Console]::Error.WriteLine($_); exit 70 }
$RunSubKey = 'Software\Microsoft\Windows\CurrentVersion\Run'

function Get-NormalDir([string] $p) {
    $p.Trim().Trim('"').TrimEnd('\').ToLowerInvariant()
}

function Add-PathEntry([string] $pathValue, [string] $dir) {
    $want = Get-NormalDir $dir
    $parts = @(if ($pathValue) { $pathValue -split ';' })
    foreach ($p in $parts) { if ($p -and (Get-NormalDir $p) -eq $want) { return $pathValue } }
    if (-not $pathValue) { return $dir }
    if ($pathValue.EndsWith(';')) { return $pathValue + $dir }
    $pathValue + ';' + $dir
}

function Remove-PathEntry([string] $pathValue, [string] $dir) {
    if (-not $pathValue) { return $pathValue }
    $want = Get-NormalDir $dir
    $kept = @($pathValue -split ';' | Where-Object { -not ($_ -and (Get-NormalDir $_) -eq $want) })
    $kept -join ';'
}

function Open-Key([string] $psPath, [bool] $write) {
    if ($psPath -match '^(HKCU|HKLM|HKU):\\(.*)$') {
        $root = switch ($Matches[1]) {
            'HKCU' { [Microsoft.Win32.Registry]::CurrentUser }
            'HKLM' { [Microsoft.Win32.Registry]::LocalMachine }
            'HKU' { [Microsoft.Win32.Registry]::Users }
        }
        if ($write) { return $root.CreateSubKey($Matches[2]) }
        return $root.OpenSubKey($Matches[2])
    }
    throw "unsupported registry path $psPath"
}

# 0: PATH changed; 10: nothing to change (already there on add, absent on remove).
function Update-EnvPath([string] $key, [string] $dir, [bool] $add) {
    $k = Open-Key $key $true
    try {
        $old = $k.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = if ($old -ne $null) { $k.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
        if ($old -eq $null -and -not $add) { return 10 }
        $new = if ($add) { Add-PathEntry $old $dir } else { Remove-PathEntry $old $dir }
        if ($new -ceq $old -or $new -eq $null) { return 10 }
        $k.SetValue('Path', $new, $kind)
        return 0
    } finally { $k.Close() }
}

# The program a Run value starts: the first quoted part, or everything up to the first space.
function Get-RunProgram([string] $value) {
    if (-not $value) { return $null }
    $v = $value.Trim()
    if ($v.StartsWith('"')) { return $v.Substring(1, $v.IndexOf('"', 1) - 1) }
    ($v -split ' ', 2)[0]
}

function Test-InDir([string] $file, [string] $dir) {
    if (-not $file) { return $false }
    (Get-NormalDir (Split-Path -Parent $file)) -eq (Get-NormalDir $dir)
}

function Get-MySid { [Security.Principal.WindowsIdentity]::GetCurrent().User.Value }

# The person signed in at this session's desktop: the owner of its explorer.exe, else this process.
function Get-DesktopSid {
    if ($DesktopSid) { return $DesktopSid }
    $session = (Get-Process -Id $PID).SessionId
    $shell = Get-CimInstance Win32_Process -Filter "Name = 'explorer.exe' AND SessionId = $session" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($shell) {
        $owner = Invoke-CimMethod -InputObject $shell -MethodName GetOwnerSid -ErrorAction SilentlyContinue
        if ($owner -and $owner.Sid) { return $owner.Sid }
    }
    Get-MySid
}

# The desktop user's own Run key, also when an administrator over the shoulder runs this.
function Get-RunKey {
    if ($RunKey) { return $RunKey }
    $sid = Get-DesktopSid
    if ($sid -eq (Get-MySid)) { return "HKCU:\$RunSubKey" }
    "HKU:\$sid\$RunSubKey"
}

function Get-RunEntryInDir([string] $dir) {
    $k = Open-Key (Get-RunKey) $false
    if (-not $k) { return $false }
    try { Test-InDir (Get-RunProgram ($k.GetValue('Wasl'))) $dir } finally { $k.Close() }
}

function Get-ProcessesInDir([string] $name, [string] $dir, [string] $ownerSid) {
    @(Get-CimInstance Win32_Process -Filter "Name = '$name'" -ErrorAction SilentlyContinue |
        Where-Object { Test-InDir $_.ExecutablePath $dir } |
        Where-Object {
            $o = Invoke-CimMethod -InputObject $_ -MethodName GetOwnerSid -ErrorAction SilentlyContinue
            $o -and $o.Sid -eq $ownerSid
        })
}

function Invoke-MsiRemove([string] $product) {
    $mode = if ($Quiet) { '/qn' } else { '/passive' }
    $argLine = "/x $product $mode /norestart"
    if ($DryRun) { [Console]::Out.WriteLine("would run: $MsiExec $argLine"); return $true }
    $p = Start-Process $MsiExec -ArgumentList $argLine -Wait -PassThru
    # 3010 and 1641: removed, a restart finishes it. 1605: that product is no longer installed.
    @(0, 1605, 1641, 3010) -contains $p.ExitCode
}

function Stop-InDir([string] $dir) {
    $result = 0
    $owner = Get-DesktopSid
    # `wasl daemon stop` reaches the wasld of whoever runs it, so ask only when that is its owner.
    $cli = Join-Path $dir 'wasl.exe'
    $canAsk = (Test-Path $cli) -and ($owner -eq (Get-MySid))
    $daemons = @(Get-ProcessesInDir 'wasld.exe' $dir $owner)
    $apps = @(Get-ProcessesInDir 'wasl-desktop.exe' $dir $owner)
    if ($daemons.Count -gt 0) { $result = $result -bor 1 }
    if ($apps.Count -gt 0) { $result = $result -bor 2 }
    if ($DryRun) {
        foreach ($d in $daemons + $apps) { [Console]::Out.WriteLine("would stop $($d.Name) $($d.ProcessId)") }
    } else {
        if ($daemons.Count -gt 0 -and $canAsk) {
            # Ask it to stop first, so it finishes what it is writing; kill it only if it does not.
            $p = Start-Process $cli -ArgumentList 'daemon', 'stop' -WindowStyle Hidden -PassThru
            if (-not ($p.WaitForExit(10000))) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
        }
        foreach ($d in $daemons) {
            $proc = Get-Process -Id $d.ProcessId -ErrorAction SilentlyContinue
            if ($proc) {
                $waitMs = if ($canAsk) { 10000 } else { 0 }
                if (-not ($proc.WaitForExit($waitMs))) { Stop-Process -Id $d.ProcessId -Force -ErrorAction SilentlyContinue }
            }
        }
        foreach ($a in $apps) { Stop-Process -Id $a.ProcessId -Force -ErrorAction SilentlyContinue }
        foreach ($d in $daemons + $apps) {
            $proc = Get-Process -Id $d.ProcessId -ErrorAction SilentlyContinue
            if ($proc) { [void]$proc.WaitForExit(5000) }
        }
    }
    if ($MsiProduct) {
        if ($MsiProduct -notmatch '^\{[0-9A-Fa-f-]{36}\}$') { return ($result -bor 8) }
        if (-not (Invoke-MsiRemove $MsiProduct)) { $result = $result -bor 8 }
    }
    $result
}

if (-not $Command) { return }

if ($Command -eq 'is-desktop-user') {
    if ((Get-DesktopSid) -eq (Get-MySid)) { exit 0 }
    exit 1
}

if (-not $Dir) { [Console]::Error.WriteLine('missing -Dir'); exit 64 }
# installer.nsi passes -Dir "$INSTDIR\." so that a drive root does not end in \" (an escaped quote).
# A quote that still came through means the command line was split wrongly: touch nothing.
if ($Dir.Contains('"')) { [Console]::Error.WriteLine("bad -Dir $Dir"); exit 64 }
$Dir = [IO.Path]::GetFullPath($Dir)
if ($Dir.Length -gt 3) { $Dir = $Dir.TrimEnd('\') }

if (-not $EnvKey) {
    $EnvKey = if ($Scope -eq 'Machine') { 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' } else { 'HKCU:\Environment' }
}

switch ($Command) {
    'path-add' { exit (Update-EnvPath $EnvKey $Dir $true) }
    'path-remove' { [void](Update-EnvPath $EnvKey $Dir $false); exit 0 }
    'run-entry-remove' {
        $k = Open-Key (Get-RunKey) $true
        try {
            if (Test-InDir (Get-RunProgram ($k.GetValue('Wasl'))) $Dir) { $k.DeleteValue('Wasl') }
        } finally { $k.Close() }
        exit 0
    }
    'component-state' {
        $state = 0
        if ($Link -and (Test-Path -LiteralPath $Link)) {
            $target = (New-Object -ComObject WScript.Shell).CreateShortcut($Link).TargetPath
            if (Test-InDir $target $Dir) { $state = $state -bor 1 }
        }
        if (Get-RunEntryInDir $Dir) { $state = $state -bor 2 }
        exit $state
    }
    'stop-in-dir' { exit (Stop-InDir $Dir) }
    default { [Console]::Error.WriteLine("unknown command $Command"); exit 64 }
}
