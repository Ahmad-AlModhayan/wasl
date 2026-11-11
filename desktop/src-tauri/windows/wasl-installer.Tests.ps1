# Tests for wasl-installer.ps1, the helper the NSIS installer runs. Pester 3.4 (built into Windows):
#   powershell -NoProfile -ExecutionPolicy Bypass -Command "Invoke-Pester desktop/src-tauri/windows"
# They drive the script the way the installer does, through its commands, against a throwaway
# registry key under HKCU\Software\WaslInstallerTest, never the real Environment or Run keys.

$script = Join-Path $PSScriptRoot 'wasl-installer.ps1'

function New-TestKey {
    $key = "HKCU:\Software\WaslInstallerTest\$([guid]::NewGuid())"
    New-Item -Path $key -Force | Out-Null
    $key
}

function Set-RawPath($key, $value, $kind = 'ExpandString') {
    $k = (Get-Item $key).OpenSubKey('', $true)
    if ($k -eq $null) { $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey(($key -replace '^HKCU:\\', ''), $true) }
    $k.SetValue('Path', $value, [Microsoft.Win32.RegistryValueKind]::$kind)
    $k.Close()
}

function Get-RawPath($key) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey(($key -replace '^HKCU:\\', ''))
    $v = $k.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $kind = if ($v -ne $null) { $k.GetValueKind('Path') } else { $null }
    $k.Close()
    @{ Value = $v; Kind = "$kind" }
}

function Invoke-Helper {
    & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $script @args | Out-Null
    $LASTEXITCODE
}

Describe 'path-add' {
    It 'appends the install folder to the existing PATH and keeps it expandable' {
        $key = New-TestKey
        Set-RawPath $key '%USERPROFILE%\bin;C:\Tools'
        Invoke-Helper path-add -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        $p = Get-RawPath $key
        $p.Value | Should Be '%USERPROFILE%\bin;C:\Tools;D:\Apps\Wasl'
        $p.Kind | Should Be 'ExpandString'
    }

    It 'creates PATH when the user has none' {
        $key = New-TestKey
        Invoke-Helper path-add -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        (Get-RawPath $key).Value | Should Be 'D:\Apps\Wasl'
    }

    It 'does not add the folder twice, whatever its case or trailing backslash' {
        $key = New-TestKey
        Set-RawPath $key 'C:\Tools;d:\apps\wasl\'
        Invoke-Helper path-add -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 10
        (Get-RawPath $key).Value | Should Be 'C:\Tools;d:\apps\wasl\'
    }

    It 'does not leave a double separator after a trailing semicolon' {
        $key = New-TestKey
        Set-RawPath $key 'C:\Tools;'
        Invoke-Helper path-add -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        (Get-RawPath $key).Value | Should Be 'C:\Tools;D:\Apps\Wasl'
    }
}

Describe 'path-remove' {
    It 'removes every copy of the install folder and leaves the rest as it was' {
        $key = New-TestKey
        Set-RawPath $key '%USERPROFILE%\bin;D:\Apps\Wasl;C:\Tools;;d:\apps\wasl\'
        Invoke-Helper path-remove -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        $p = Get-RawPath $key
        $p.Value | Should Be '%USERPROFILE%\bin;C:\Tools;'
        $p.Kind | Should Be 'ExpandString'
    }

    It 'does not remove a folder that only starts with the install folder' {
        $key = New-TestKey
        Set-RawPath $key 'D:\Apps\Wasl Tools;D:\Apps\Wasl'
        Invoke-Helper path-remove -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        (Get-RawPath $key).Value | Should Be 'D:\Apps\Wasl Tools'
    }

    It 'succeeds when there is no PATH at all' {
        $key = New-TestKey
        Invoke-Helper path-remove -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        (Get-RawPath $key).Value | Should Be $null
    }
}

Describe 'run-entry-remove' {
    function Set-Run($key, $value) { Set-ItemProperty -Path $key -Name 'Wasl' -Value $value }
    function Get-Run($key) { (Get-ItemProperty -Path $key -Name 'Wasl' -ErrorAction SilentlyContinue).Wasl }

    It 'removes the sign-in entry when it starts wasld from this install' {
        $key = New-TestKey
        Set-Run $key '"D:\Apps\Wasl\wasld.exe"'
        Invoke-Helper run-entry-remove -Dir 'D:\Apps\Wasl\' -RunKey $key | Should Be 0
        Get-Run $key | Should Be $null
    }

    It 'keeps the sign-in entry of another Wasl install' {
        $key = New-TestKey
        Set-Run $key '"C:\Program Files\Wasl\wasld.exe"'
        Invoke-Helper run-entry-remove -Dir 'D:\Apps\Wasl' -RunKey $key | Should Be 0
        Get-Run $key | Should Be '"C:\Program Files\Wasl\wasld.exe"'
    }

    It 'keeps an entry from a folder whose name only starts with the install folder' {
        $key = New-TestKey
        Set-Run $key '"D:\Apps\Wasl2\wasld.exe"'
        Invoke-Helper run-entry-remove -Dir 'D:\Apps\Wasl' -RunKey $key | Should Be 0
        Get-Run $key | Should Be '"D:\Apps\Wasl2\wasld.exe"'
    }
}

Describe 'stop-in-dir' {
    It 'stops wasld running from this install, reports it, and leaves other processes alone' {
        $dir = Join-Path $env:TEMP "wasl-stop-$([guid]::NewGuid())"
        $other = Join-Path $env:TEMP "wasl-other-$([guid]::NewGuid())"
        New-Item -ItemType Directory $dir, $other | Out-Null
        # Stand-ins called wasld.exe that only sleep, one in this install and one elsewhere.
        $sleeper = 'class P { static void Main() { System.Threading.Thread.Sleep(60000); } }'
        Add-Type -TypeDefinition $sleeper -OutputType ConsoleApplication -OutputAssembly (Join-Path $dir 'wasld.exe')
        Copy-Item (Join-Path $dir 'wasld.exe') (Join-Path $other 'wasld.exe')
        $mine = Start-Process (Join-Path $dir 'wasld.exe') -WindowStyle Hidden -PassThru
        $theirs = Start-Process (Join-Path $other 'wasld.exe') -WindowStyle Hidden -PassThru
        Start-Sleep -Seconds 1 # until Windows reports their image paths
        try {
            # Exit code bit 1: wasld was running from this install.
            Invoke-Helper stop-in-dir -Dir $dir | Should Be 1
            $mine.WaitForExit(5000) | Should Be $true
            $theirs.HasExited | Should Be $false
        } finally {
            Stop-Process -Id $mine.Id, $theirs.Id -Force -ErrorAction SilentlyContinue
            Start-Sleep -Milliseconds 300
            Remove-Item -Recurse -Force $dir, $other -ErrorAction SilentlyContinue
        }
    }

    It 'reports 0 when nothing runs from this install' {
        $dir = Join-Path $env:TEMP "wasl-none-$([guid]::NewGuid())"
        Invoke-Helper stop-in-dir -Dir $dir | Should Be 0
    }
}

Describe 'PATH ownership and long values' {
    It 'tells the installer when the folder was already on PATH, so uninstall leaves it' {
        $key = New-TestKey
        Set-RawPath $key 'C:\Tools;D:\Apps\Wasl'
        # 10: already there, nothing changed; the installer then records that it did not add it.
        Invoke-Helper path-add -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 10
        (Get-RawPath $key).Value | Should Be 'C:\Tools;D:\Apps\Wasl'
    }

    It 'keeps a PATH longer than 1024 characters whole when adding and removing' {
        $key = New-TestKey
        $long = (1..60 | ForEach-Object { "C:\Some\Rather\Long\Tool\Folder\$_" }) -join ';'
        $long.Length | Should BeGreaterThan 1024
        Set-RawPath $key $long
        Invoke-Helper path-add -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        (Get-RawPath $key).Value | Should Be "$long;D:\Apps\Wasl"
        Invoke-Helper path-remove -Dir 'D:\Apps\Wasl' -EnvKey $key | Should Be 0
        (Get-RawPath $key).Value | Should Be $long
    }
}

Describe 'install folder argument' {
    It 'takes a drive root passed the way the installer quotes it' {
        $key = New-TestKey
        Set-RawPath $key 'C:\Tools'
        # installer.nsi passes -Dir "$INSTDIR\.", so a root (D:\) never ends in \" on the command line.
        $cmd = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$script`" path-add -EnvKey `"$key`" -Dir `"D:\\.`""
        $p = Start-Process powershell.exe -ArgumentList $cmd -Wait -PassThru -WindowStyle Hidden
        $p.ExitCode | Should Be 0
        (Get-RawPath $key).Value | Should Be 'C:\Tools;D:\'
    }

    It 'refuses a folder that came through with a quote in it instead of writing it to PATH' {
        $key = New-TestKey
        Set-RawPath $key 'C:\Tools'
        # What -Dir "D:\" turns into: the \" escapes the closing quote.
        $cmd = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$script`" path-add -EnvKey `"$key`" -Dir `"D:\`""
        $p = Start-Process powershell.exe -ArgumentList $cmd -Wait -PassThru -WindowStyle Hidden
        $p.ExitCode | Should Be 64
        (Get-RawPath $key).Value | Should Be 'C:\Tools'
    }
}

function New-Sleeper([string] $path) {
    $sleeper = 'class P { static void Main() { System.Threading.Thread.Sleep(60000); } }'
    Add-Type -TypeDefinition $sleeper -OutputType ConsoleApplication -OutputAssembly $path
}

Describe 'stop-in-dir and the .msi migration' {
    # A fake install laid out like this PC: an all-users .msi (0.3.3) whose InstallLocation is the same
    # folder the live NSIS install runs wasld from. The .msi must go only after that wasld is stopped.
    $fakeMsi = @'
using System; using System.IO; using System.Diagnostics;
class M { static int Main(string[] a) {
  int pid = int.Parse(Environment.GetEnvironmentVariable("WASL_FAKE_WATCH"));
  bool alive; try { alive = !Process.GetProcessById(pid).HasExited; } catch (ArgumentException) { alive = false; }
  File.WriteAllText(Environment.GetEnvironmentVariable("WASL_FAKE_LOG"), "args=" + string.Join(" ", a) + "\nwasld-alive=" + alive);
  return int.Parse(Environment.GetEnvironmentVariable("WASL_FAKE_EXIT")); } }
'@
    $root = Join-Path $env:TEMP "wasl-msi-$([guid]::NewGuid())"
    $dir = Join-Path $root 'Program Files\Wasl'
    New-Item -ItemType Directory $dir | Out-Null
    $msiexec = Join-Path $root 'msiexec.exe'
    Add-Type -TypeDefinition $fakeMsi -OutputType ConsoleApplication -OutputAssembly $msiexec
    New-Sleeper (Join-Path $dir 'wasld.exe')
    $product = '{CE46A1D7-86CB-4C41-916C-0D64D7564154}'
    $env:WASL_FAKE_LOG = Join-Path $root 'msiexec.log'

    function Start-Wasld {
        $p = Start-Process (Join-Path $dir 'wasld.exe') -WindowStyle Hidden -PassThru
        Start-Sleep -Seconds 1
        $env:WASL_FAKE_WATCH = "$($p.Id)"
        $p
    }

    It 'stops this install''s wasld before it removes the .msi, and takes 3010 as success' {
        $env:WASL_FAKE_EXIT = '3010'
        $w = Start-Wasld
        try {
            Invoke-Helper stop-in-dir -Dir $dir -MsiProduct $product -MsiExec $msiexec -Quiet | Should Be 1
            $log = Get-Content $env:WASL_FAKE_LOG -Raw
            $log | Should Match 'wasld-alive=False'
            $log | Should Match ([regex]::Escape("/x $product"))
            $log | Should Match '/qn'
            $log | Should Match '/norestart'
        } finally { Stop-Process -Id $w.Id -Force -ErrorAction SilentlyContinue }
    }

    It 'reports a failed .msi removal (bit 8) so the installer stops before copying files' {
        $env:WASL_FAKE_EXIT = '1603'
        $w = Start-Wasld
        try {
            Invoke-Helper stop-in-dir -Dir $dir -MsiProduct $product -MsiExec $msiexec | Should Be 9
        } finally { Stop-Process -Id $w.Id -Force -ErrorAction SilentlyContinue }
    }

    It 'in a dry run, stops nothing and removes nothing, and says what it would do' {
        Remove-Item $env:WASL_FAKE_LOG -ErrorAction SilentlyContinue
        $w = Start-Wasld
        try {
            $out = & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $script stop-in-dir -Dir $dir -MsiProduct $product -MsiExec $msiexec -DryRun
            $LASTEXITCODE | Should Be 1
            ($out -join "`n") | Should Match "would stop wasld.exe $($w.Id)"
            ($out -join "`n") | Should Match ([regex]::Escape("would run: $msiexec /x $product"))
            $w.HasExited | Should Be $false
            Test-Path $env:WASL_FAKE_LOG | Should Be $false
        } finally { Stop-Process -Id $w.Id -Force -ErrorAction SilentlyContinue }
    }

    It 'leaves a wasld that belongs to another user alone' {
        $w = Start-Wasld
        try {
            # A SID that owns nothing here stands in for the person at the desktop being someone else.
            Invoke-Helper stop-in-dir -Dir $dir -DesktopSid 'S-1-5-21-1-2-3-1001' | Should Be 0
            $w.HasExited | Should Be $false
        } finally { Stop-Process -Id $w.Id -Force -ErrorAction SilentlyContinue }
    }

    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}

Describe 'component-state' {
    # What an earlier install left behind, for upgrades from builds that stored no choices (0.4.0).
    function New-Link([string] $lnk, [string] $target) {
        $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk); $s.TargetPath = $target; $s.Save()
    }
    $root = Join-Path $env:TEMP "wasl-state-$([guid]::NewGuid())"
    $dir = Join-Path $root 'Wasl'
    New-Item -ItemType Directory $dir | Out-Null

    It 'reports a desktop shortcut (1) and a sign-in entry (2) that point at this install' {
        $key = New-TestKey
        Set-ItemProperty -Path $key -Name 'Wasl' -Value "`"$dir\wasld.exe`""
        New-Link "$root\Wasl.lnk" "$dir\wasl-desktop.exe"
        Invoke-Helper component-state -Dir $dir -Link "$root\Wasl.lnk" -RunKey $key | Should Be 3
    }

    It 'reports nothing for a shortcut and an entry that point at another copy' {
        $key = New-TestKey
        Set-ItemProperty -Path $key -Name 'Wasl' -Value '"C:\Elsewhere\Wasl\wasld.exe"'
        New-Link "$root\Other.lnk" 'C:\Elsewhere\Wasl\wasl-desktop.exe'
        Invoke-Helper component-state -Dir $dir -Link "$root\Other.lnk" -RunKey $key | Should Be 0
    }

    It 'reports nothing when there is neither' {
        $key = New-TestKey
        Invoke-Helper component-state -Dir $dir -Link "$root\missing.lnk" -RunKey $key | Should Be 0
    }

    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}

Describe 'is-desktop-user' {
    It 'says 0 when the person at the desktop is running the installer' {
        Invoke-Helper is-desktop-user | Should Be 0
    }
    It 'says 1 for someone else (an administrator who elevated it over the shoulder)' {
        Invoke-Helper is-desktop-user -DesktopSid 'S-1-5-21-1-2-3-1001' | Should Be 1
    }
}

# wix-entry.nsh is NSIS, not the helper: it is built with makensis (the copy Tauri downloads, or
# $env:WASL_MAKENSIS) into a silent, unelevated test program whose exit code is the macro's answer.
Describe 'WaslIsWixEntry (the old .msi lookup)' {
    $makensis = if ($env:WASL_MAKENSIS) { $env:WASL_MAKENSIS } else { "$env:LOCALAPPDATA\tauri\NSIS\makensis.exe" }
    $noNsis = -not (Test-Path $makensis)
    $exe = Join-Path $env:TEMP "wasl-wix-entry-$([guid]::NewGuid()).exe"
    if (-not $noNsis) {
        $nsh = Join-Path $PSScriptRoot 'wix-entry.nsh'
        $nsi = [IO.Path]::ChangeExtension($exe, '.nsi')
        @"
Unicode true
OutFile "$exe"
RequestExecutionLevel user
SilentInstall silent
!include LogicLib.nsh
!include FileFunc.nsh
!include StrFunc.nsh
`${StrCase}
`${StrLoc}
!include "$nsh"
Section
  `${GetParameters} `$2
  !insertmacro WaslIsWixEntry HKCU `$2 "Wasl" "kahflabs" `$3
  SetErrorLevel `$3
SectionEnd
"@ | Set-Content -Path $nsi -Encoding UTF8
        & $makensis /V1 $nsi | Out-Null
        Remove-Item $nsi -ErrorAction SilentlyContinue
    }

    function Test-Entry([string] $name, [string] $publisher, [string] $uninstall) {
        $key = New-TestKey
        New-ItemProperty $key DisplayName -Value $name | Out-Null
        New-ItemProperty $key Publisher -Value $publisher | Out-Null
        New-ItemProperty $key UninstallString -Value $uninstall | Out-Null
        $p = Start-Process $exe -ArgumentList ($key -replace '^HKCU:\\', '') -Wait -PassThru -WindowStyle Hidden
        $p.ExitCode
    }
    $msi = 'MsiExec.exe /X{11111111-2222-3333-4444-555555555555}'

    It 'finds the .msi entry when DisplayName is Wasl and Publisher is kahflabs' -Skip:$noNsis {
        Test-Entry 'Wasl' 'kahflabs' $msi | Should Be 1
    }
    It 'does not take a name and publisher that only join up to the same text' -Skip:$noNsis {
        Test-Entry 'Waslk' 'ahflabs' $msi | Should Be 0
        Test-Entry 'Was' 'lkahflabs' $msi | Should Be 0
    }
    It 'does not take another publisher''s Wasl' -Skip:$noNsis {
        Test-Entry 'Wasl' 'someone else' $msi | Should Be 0
    }
    It 'does not take an entry that is not uninstalled by msiexec' -Skip:$noNsis {
        Test-Entry 'Wasl' 'kahflabs' '"C:\Program Files\Wasl\uninstall.exe"' | Should Be 0
    }

    Remove-Item $exe -ErrorAction SilentlyContinue
}

Remove-Item -Recurse -Force 'HKCU:\Software\WaslInstallerTest' -ErrorAction SilentlyContinue
