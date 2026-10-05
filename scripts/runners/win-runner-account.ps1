<#
.SYNOPSIS
  #219: run the trellis-win self-hosted runner under a low-privilege local
  account, and keep the trellis-wsl runner's distro alive for its systemd unit.

.DESCRIPTION
  Run elevated, after scripts/runners/wsl-runner-account.sh (see
  docs/BUILD_SYSTEM.md "CI runners"). Each step prints what it does; -WhatIf
  prints what it would do and changes nothing. Safe to run again.

  1. Creates the standard (non-admin) local user trellis-ci-win, a member of
     Users only, hidden from the sign-in screen, denied interactive, remote
     interactive and network logon, granted "log on as a batch job".
  2. Saves the runner directory's ACLs, then gives the directory to that user:
     owner trellis-ci-win, access SYSTEM + Administrators + trellis-ci-win only.
  3. Adds explicit Deny ACEs for that user on the owner's own trees that
     C:\'s default ACL leaves writable by every signed-in user (C:\w,
     C:\ci-local by default), and lists any other such folder.
  4. Puts the runner's own cargo\bin on PATH in start-runner.cmd (the
     provider-key scrub lines are kept; the script refuses if they are gone).
  5. Re-registers the trellis-runner-win task to run as that user at startup,
     whether anyone is logged on or not.
  6. Re-registers trellis-runner-wsl as a keep-alive for the WSL distro (the
     runner itself is now the trellis-runner-wsl systemd unit, as trellis-ci).

  The account's password is random, set by this script, handed only to Task
  Scheduler, and never printed or stored anywhere else. A re-run resets it.

  -Rollback undoes steps 2-6 and disables the account (it is not deleted).

.EXAMPLE
  pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\runners\win-runner-account.ps1 -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$UserName = 'trellis-ci-win',
    [string]$RunnerDir = 'C:\actions-runner\win',
    [string]$TaskName = 'trellis-runner-win',
    [string]$WslTaskName = 'trellis-runner-wsl',
    [string]$WslDistro = 'Ubuntu',
    [string]$WslOldStart = '/home/mayda/actions-runner/start.sh',
    [string[]]$DenyPaths = @('C:\w', 'C:\ci-local'),
    # The interactive account that owns the machine (gets the WSL keep-alive
    # task and, on -Rollback, the runner directory back). Default: whoever
    # runs this script elevated.
    [string]$OwnerUser = '',
    [switch]$OwnerCanRead,
    [switch]$SkipWsl,
    [switch]$StopRunner,
    [switch]$Rollback
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# --- refuse without admin, before anything else ------------------------------
$me = [Security.Principal.WindowsIdentity]::GetCurrent()
$isAdmin = ([Security.Principal.WindowsPrincipal]$me).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    [Console]::Error.WriteLine(
        "refusing: this script changes accounts, ACLs and scheduled tasks; " +
        "run it from an elevated PowerShell (Run as administrator). Nothing was changed.")
    exit 1
}

if (-not $OwnerUser) { $OwnerUser = $me.Name }
$Qualified = "$env:COMPUTERNAME\$UserName"
$AclBackup = "$RunnerDir.acl-pre219.txt"
$StartCmd = Join-Path $RunnerDir 'start-runner.cmd'
$StartBackup = "$StartCmd.pre219"
$SidSystem = 'S-1-5-18'
$SidAdmins = 'S-1-5-32-544'
$SidUsers = 'S-1-5-32-545'
$SidAuthUsers = 'S-1-5-11'

$script:StepNo = 0
function Step([string]$What) {
    $script:StepNo++
    Write-Host ("[{0}] {1}" -f $script:StepNo, $What)
}
function Note([string]$What) { Write-Host "    $What" }

function Invoke-Native([string]$Exe, [string[]]$Arguments) {
    Note ("+ {0} {1}" -f $Exe, ($Arguments -join ' '))
    & $Exe @Arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "$Exe exited $LASTEXITCODE" }
}

function New-RunnerPassword {
    $bytes = New-Object byte[] 30
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
    # Prefix guarantees the complexity classes; the rest is 240 random bits.
    return 'Tr1!' + [Convert]::ToBase64String($bytes)
}

function Get-UserSid {
    $u = Get-LocalUser -Name $UserName -ErrorAction SilentlyContinue
    if ($u) { return $u.SID.Value }
    return $null
}

# --- user rights through the LSA (additive; no secedit merge needed) ----------
if (-not ('Trellis219.Lsa' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Security.Principal;
namespace Trellis219 {
public static class Lsa {
    [StructLayout(LayoutKind.Sequential)]
    struct LSA_UNICODE_STRING { public ushort Length; public ushort MaximumLength; public IntPtr Buffer; }
    [StructLayout(LayoutKind.Sequential)]
    struct LSA_OBJECT_ATTRIBUTES { public int Length; public IntPtr RootDirectory; public IntPtr ObjectName; public uint Attributes; public IntPtr SecurityDescriptor; public IntPtr SecurityQualityOfService; }
    [DllImport("advapi32.dll")] static extern uint LsaOpenPolicy(IntPtr systemName, ref LSA_OBJECT_ATTRIBUTES attrs, uint access, out IntPtr handle);
    [DllImport("advapi32.dll")] static extern uint LsaAddAccountRights(IntPtr handle, byte[] sid, LSA_UNICODE_STRING[] rights, uint count);
    [DllImport("advapi32.dll")] static extern uint LsaRemoveAccountRights(IntPtr handle, byte[] sid, bool all, LSA_UNICODE_STRING[] rights, uint count);
    [DllImport("advapi32.dll")] static extern uint LsaEnumerateAccountRights(IntPtr handle, byte[] sid, out IntPtr rights, out uint count);
    [DllImport("advapi32.dll")] static extern uint LsaClose(IntPtr handle);
    [DllImport("advapi32.dll")] static extern uint LsaFreeMemory(IntPtr p);
    [DllImport("advapi32.dll")] static extern int LsaNtStatusToWinError(uint status);
    const uint POLICY_ALL_ACCESS = 0x000F0FFF;
    const uint STATUS_OBJECT_NAME_NOT_FOUND = 0xC0000034;

    static IntPtr Open() {
        var attrs = new LSA_OBJECT_ATTRIBUTES();
        IntPtr h;
        uint st = LsaOpenPolicy(IntPtr.Zero, ref attrs, POLICY_ALL_ACCESS, out h);
        if (st != 0) throw new System.ComponentModel.Win32Exception(LsaNtStatusToWinError(st));
        return h;
    }
    static byte[] SidBytes(string sid) {
        var s = new SecurityIdentifier(sid);
        var b = new byte[s.BinaryLength];
        s.GetBinaryForm(b, 0);
        return b;
    }
    static LSA_UNICODE_STRING Str(string v) {
        var u = new LSA_UNICODE_STRING();
        u.Buffer = Marshal.StringToHGlobalUni(v);
        u.Length = (ushort)(v.Length * 2);
        u.MaximumLength = (ushort)(u.Length + 2);
        return u;
    }
    public static string[] Get(string sid) {
        IntPtr h = Open();
        try {
            IntPtr p; uint n;
            uint st = LsaEnumerateAccountRights(h, SidBytes(sid), out p, out n);
            if (st == STATUS_OBJECT_NAME_NOT_FOUND) return new string[0];
            if (st != 0) throw new System.ComponentModel.Win32Exception(LsaNtStatusToWinError(st));
            var list = new List<string>();
            int size = Marshal.SizeOf(typeof(LSA_UNICODE_STRING));
            for (int i = 0; i < n; i++) {
                var u = (LSA_UNICODE_STRING)Marshal.PtrToStructure(new IntPtr(p.ToInt64() + i * size), typeof(LSA_UNICODE_STRING));
                list.Add(Marshal.PtrToStringUni(u.Buffer, u.Length / 2));
            }
            LsaFreeMemory(p);
            return list.ToArray();
        } finally { LsaClose(h); }
    }
    public static void Set(string sid, string right, bool add) {
        IntPtr h = Open();
        var u = Str(right);
        try {
            var arr = new LSA_UNICODE_STRING[] { u };
            uint st = add ? LsaAddAccountRights(h, SidBytes(sid), arr, 1)
                          : LsaRemoveAccountRights(h, SidBytes(sid), false, arr, 1);
            if (st != 0 && !(st == STATUS_OBJECT_NAME_NOT_FOUND && !add))
                throw new System.ComponentModel.Win32Exception(LsaNtStatusToWinError(st));
        } finally { Marshal.FreeHGlobal(u.Buffer); LsaClose(h); }
    }
}
}
'@
}

$Rights = @(
    'SeDenyInteractiveLogonRight',
    'SeDenyRemoteInteractiveLogonRight',
    'SeDenyNetworkLogonRight',
    'SeBatchLogonRight'
)

function Assert-RunnerStopped {
    $running = @(Get-Process -Name 'Runner.Listener', 'Runner.Worker' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($RunnerDir, [StringComparison]::OrdinalIgnoreCase) })
    if ($running.Count -eq 0) { Note 'runner is not running'; return }
    Note ("runner is running (pid {0})" -f (($running | ForEach-Object Id) -join ', '))
    if (-not $StopRunner) {
        if ($WhatIfPreference) { Note 'a real run would refuse here: stop it first or pass -StopRunner'; return }
        throw "refusing: the runner in $RunnerDir is running. Stop it (schtasks /end /tn $TaskName, then wait for Runner.Listener to exit) or pass -StopRunner, which also cancels a job in progress."
    }
    if ($PSCmdlet.ShouldProcess($TaskName, 'end the task and stop the runner processes')) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        $running | Stop-Process -Force
        Note 'stopped'
    }
}

function Register-OwnerTask([string]$Name, $Action, [string]$Description) {
    $principal = New-ScheduledTaskPrincipal -UserId $OwnerUser -LogonType Interactive -RunLevel Limited
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $OwnerUser
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) `
        -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskName $Name -Action $Action -Trigger $trigger -Principal $principal `
        -Settings $settings -Description $Description -Force | Out-Null
}

# =============================================================================
if ($Rollback) {
    Write-Host "#219 rollback: $TaskName back to $OwnerUser at logon; $UserName disabled"

    Step 'runner must be stopped'
    Assert-RunnerStopped

    Step "restore ACLs of $RunnerDir"
    if (Test-Path -LiteralPath $AclBackup) {
        if ($PSCmdlet.ShouldProcess($RunnerDir, "icacls /restore from $AclBackup, owner back to $OwnerUser")) {
            Invoke-Native 'icacls.exe' @($RunnerDir, '/reset')
            Invoke-Native 'icacls.exe' @((Split-Path -Parent $RunnerDir), '/restore', $AclBackup, '/C', '/Q')
            Invoke-Native 'icacls.exe' @($RunnerDir, '/setowner', $OwnerUser, '/T', '/C', '/Q')
        }
    } else { Note "no ${AclBackup}: nothing to restore" }

    Step 'remove the Deny ACEs'
    $sid = Get-UserSid
    foreach ($p in $DenyPaths) {
        if ($sid -and (Test-Path -LiteralPath $p)) {
            if ($PSCmdlet.ShouldProcess($p, "remove Deny ACEs for $UserName")) {
                Invoke-Native 'icacls.exe' @($p, '/remove:d', "*$sid", '/C', '/Q')
            }
        }
    }

    Step 'restore start-runner.cmd'
    if (Test-Path -LiteralPath $StartBackup) {
        if ($PSCmdlet.ShouldProcess($StartCmd, "copy back $StartBackup")) {
            Copy-Item -LiteralPath $StartBackup -Destination $StartCmd -Force
        }
    } else { Note "no ${StartBackup}: unchanged" }

    Step "re-register $TaskName as $OwnerUser at logon (the pre-#219 task)"
    if ($PSCmdlet.ShouldProcess($TaskName, 'register')) {
        $a = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument "/c `"$StartCmd`"" -WorkingDirectory $RunnerDir
        Register-OwnerTask $TaskName $a 'Starts the trellis-win GitHub Actions runner at logon (interim until #219 moves it to a low-privilege account).'
    }

    if (-not $SkipWsl) {
        Step "re-register $WslTaskName as $OwnerUser at logon (the pre-#219 task)"
        if ($PSCmdlet.ShouldProcess($WslTaskName, 'register')) {
            $a = New-ScheduledTaskAction -Execute 'wsl.exe' -Argument "-d $WslDistro -- $WslOldStart"
            Register-OwnerTask $WslTaskName $a 'Starts the trellis-wsl GitHub Actions runner at logon (interim until #219).'
        }
    }

    Step "disable $UserName and drop its logon rights"
    $sid = Get-UserSid
    if ($sid) {
        if ($PSCmdlet.ShouldProcess($UserName, 'disable account, remove rights')) {
            Disable-LocalUser -Name $UserName
            foreach ($r in $Rights) { [Trellis219.Lsa]::Set($sid, $r, $false) }
        }
        Note "to delete it as well: Remove-LocalUser $UserName; then delete C:\Users\$UserName"
    } else { Note 'no such account' }

    Write-Host 'done. Start it again with: schtasks /run /tn trellis-runner-win'
    exit 0
}

# =============================================================================
Write-Host "#219: $TaskName runs as $Qualified (standard user, no interactive logon)"
if ($WhatIfPreference) { Write-Host '(-WhatIf: nothing below is changed)' }

if (-not (Test-Path -LiteralPath $RunnerDir)) { throw "no runner at $RunnerDir" }
if (-not (Test-Path -LiteralPath (Join-Path $RunnerDir '.runner'))) { throw "$RunnerDir is not a configured runner (no .runner)" }

Step 'runner must be stopped'
Assert-RunnerStopped

# The password exists only in this process; Task Scheduler stores it (LSA).
$plain = $null
if (-not $WhatIfPreference) { $plain = New-RunnerPassword }

Step "local user $UserName"
$user = Get-LocalUser -Name $UserName -ErrorAction SilentlyContinue
if (-not $user) {
    if ($PSCmdlet.ShouldProcess($UserName, 'create standard local user (random password, never expires)')) {
        $sec = ConvertTo-SecureString $plain -AsPlainText -Force
        New-LocalUser -Name $UserName -Password $sec -FullName 'trellis CI runner' `
            -Description 'Runs the trellis-win GitHub Actions runner (#219). No interactive logon.' `
            -PasswordNeverExpires -UserMayNotChangePassword -AccountNeverExpires | Out-Null
        Note 'created'
    }
} else {
    Note 'exists'
    if ($PSCmdlet.ShouldProcess($UserName, 'enable and set a fresh random password (handed to Task Scheduler below)')) {
        $sec = ConvertTo-SecureString $plain -AsPlainText -Force
        Set-LocalUser -Name $UserName -Password $sec -PasswordNeverExpires $true
        Enable-LocalUser -Name $UserName
    }
}
$sid = Get-UserSid

Step 'group membership: Users only'
$users = Get-LocalGroup -SID $SidUsers
$admins = Get-LocalGroup -SID $SidAdmins
if ($sid) {
    $inUsers = @(Get-LocalGroupMember -Group $users | Where-Object { $_.SID.Value -eq $sid }).Count -gt 0
    $inAdmins = @(Get-LocalGroupMember -Group $admins | Where-Object { $_.SID.Value -eq $sid }).Count -gt 0
} else { $inUsers = $false; $inAdmins = $false }
if (-not $inUsers) {
    if ($PSCmdlet.ShouldProcess($users.Name, "add $UserName")) { Add-LocalGroupMember -Group $users -Member $UserName }
} else { Note "already in $($users.Name)" }
if ($inAdmins) {
    if ($PSCmdlet.ShouldProcess($admins.Name, "remove $UserName")) { Remove-LocalGroupMember -Group $admins -Member $UserName }
} else { Note "not in $($admins.Name)" }

Step 'logon rights: deny interactive, remote interactive and network; allow batch (Task Scheduler)'
$have = @()
if ($sid) { $have = [Trellis219.Lsa]::Get($sid) }
foreach ($r in $Rights) {
    if ($have -contains $r) { Note "has $r"; continue }
    if ($PSCmdlet.ShouldProcess($UserName, "grant $r")) { [Trellis219.Lsa]::Set($sid, $r, $true); Note "granted $r" }
}

Step 'hide the account from the sign-in screen'
$ul = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList'
$cur = $null
if (Test-Path $ul) { $cur = (Get-ItemProperty -Path $ul -Name $UserName -ErrorAction SilentlyContinue) }
if ($cur -and $cur.$UserName -eq 0) { Note 'already hidden' }
elseif ($PSCmdlet.ShouldProcess($ul, "set $UserName = 0")) {
    if (-not (Test-Path $ul)) { New-Item -Path $ul -Force | Out-Null }
    New-ItemProperty -Path $ul -Name $UserName -Value 0 -PropertyType DWord -Force | Out-Null
}

Step "save the current ACLs of $RunnerDir (the rollback)"
if (Test-Path -LiteralPath $AclBackup) { Note "kept existing $AclBackup (first run's)" }
elseif ($PSCmdlet.ShouldProcess($AclBackup, 'icacls /save /T')) {
    Invoke-Native 'icacls.exe' @($RunnerDir, '/save', $AclBackup, '/T', '/C', '/Q')
}

Step "give $RunnerDir to ${UserName}: SYSTEM, Administrators and $UserName only"
$grants = @("*${SidSystem}:(OI)(CI)F", "*${SidAdmins}:(OI)(CI)F", "${Qualified}:(OI)(CI)M")
if ($OwnerCanRead) { $grants += "*$($me.User.Value):(OI)(CI)RX" }
if ($PSCmdlet.ShouldProcess($RunnerDir, 'replace the ACL, set owner, children inherit')) {
    Invoke-Native 'icacls.exe' @($RunnerDir, '/reset')
    Invoke-Native 'icacls.exe' (@($RunnerDir, '/inheritance:r', '/grant:r') + $grants)
    Invoke-Native 'icacls.exe' @((Join-Path $RunnerDir '*'), '/reset', '/T', '/C', '/Q')
    Invoke-Native 'icacls.exe' @($RunnerDir, '/setowner', $Qualified, '/T', '/C', '/Q')
}
# The registration key and token: never readable by the owner's own processes.
foreach ($f in @('.credentials', '.credentials_rsaparams')) {
    $p = Join-Path $RunnerDir $f
    if (-not (Test-Path -LiteralPath $p)) { continue }
    if ($PSCmdlet.ShouldProcess($p, 'SYSTEM, Administrators, runner user only')) {
        Invoke-Native 'icacls.exe' @($p, '/inheritance:r', '/grant:r', "*${SidSystem}:F", "*${SidAdmins}:F", "${Qualified}:M", '/Q')
    }
}

Step "deny $UserName the owner's trees that C:\ leaves writable to every signed-in user"
foreach ($p in $DenyPaths) {
    if (-not (Test-Path -LiteralPath $p)) { Note "$p does not exist: skipped"; continue }
    $has = $false
    if ($sid) {
        $rules = (Get-Acl -LiteralPath $p).GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier])
        $has = @($rules | Where-Object { $_.IdentityReference.Value -eq $sid -and $_.AccessControlType -eq 'Deny' }).Count -gt 0
    }
    if ($has) { Note "$p already denies $UserName"; continue }
    if ($PSCmdlet.ShouldProcess($p, "add Deny (OI)(CI)F for $UserName")) {
        Invoke-Native 'icacls.exe' @($p, '/deny', "${Qualified}:(OI)(CI)F", '/Q')
    }
}
# Report-only: other top-level folders a standard user can write to.
$skip = @('Windows', 'Program Files', 'Program Files (x86)', 'ProgramData', 'Users',
    '$Recycle.Bin', 'System Volume Information', 'Recovery', 'PerfLogs', 'Config.Msi')
$open = @()
foreach ($d in (Get-ChildItem -LiteralPath 'C:\' -Directory -Force -ErrorAction SilentlyContinue)) {
    if ($skip -contains $d.Name) { continue }
    if ($d.FullName.TrimEnd('\') -ieq $RunnerDir.TrimEnd('\')) { continue }
    if ($DenyPaths | Where-Object { $_.TrimEnd('\') -ieq $d.FullName.TrimEnd('\') }) { continue }
    try {
        $rules = (Get-Acl -LiteralPath $d.FullName).GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
    } catch { continue }
    $w = $rules | Where-Object {
        ($_.IdentityReference.Value -eq $SidAuthUsers -or $_.IdentityReference.Value -eq $SidUsers) -and
        $_.AccessControlType -eq 'Allow' -and
        ($_.FileSystemRights -band [Security.AccessControl.FileSystemRights]::WriteData)
    }
    if ($w) { $open += $d.FullName }
}
if ($open.Count) {
    Note ('also writable by any signed-in user (not changed; add to -DenyPaths if they hold anything of yours): ' + ($open -join ', '))
}

Step 'start-runner.cmd: runner-local cargo\bin on PATH, provider-key scrub kept'
$text = Get-Content -LiteralPath $StartCmd -Raw
foreach ($k in @('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GROQ_API_KEY', 'GEMINI_API_KEY')) {
    if ($text -notmatch "(?im)^set $k=\s*$") { throw "refusing: start-runner.cmd no longer clears $k; restore the scrub lines first" }
}
$oldLine = 'set PATH=C:\Program Files\Git\bin;%PATH%'
$newLine = 'set PATH=%~dp0cargo\bin;C:\Program Files\Git\bin;%PATH%'
if ($text.Contains($newLine)) { Note 'already done' }
elseif (-not $text.Contains($oldLine)) { Note "no '$oldLine' line: left as it is" }
elseif ($PSCmdlet.ShouldProcess($StartCmd, "prepend %~dp0cargo\bin to PATH (backup $StartBackup)")) {
    if (-not (Test-Path -LiteralPath $StartBackup)) { Copy-Item -LiteralPath $StartCmd -Destination $StartBackup }
    [IO.File]::WriteAllText($StartCmd, $text.Replace($oldLine, $newLine), [Text.Encoding]::ASCII)
}

Step "scheduled task ${TaskName}: as $Qualified, at startup, logged on or not"
if ($PSCmdlet.ShouldProcess($TaskName, "register as $Qualified (password logon, limited)")) {
    $action = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument "/c `"$StartCmd`"" -WorkingDirectory $RunnerDir
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) `
        -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew `
        -StartWhenAvailable -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
        -User $Qualified -Password $plain -RunLevel Limited -Force `
        -Description 'trellis-win GitHub Actions runner as the low-privilege trellis-ci-win account (#219).' | Out-Null
    Note 'registered'
}
$plain = $null

if (-not $SkipWsl) {
    Step "scheduled task ${WslTaskName}: keep the $WslDistro distro up for the trellis-runner-wsl unit"
    Note 'the runner runs as trellis-ci under systemd (wsl-runner-account.sh); this task only keeps the distro alive'
    if ($PSCmdlet.ShouldProcess($WslTaskName, "register as $OwnerUser at logon: wsl.exe -d $WslDistro --exec /bin/sleep infinity")) {
        $a = New-ScheduledTaskAction -Execute 'wsl.exe' -Argument "-d $WslDistro --exec /bin/sleep infinity"
        Register-OwnerTask $WslTaskName $a 'Keeps the WSL distro running so the trellis-runner-wsl systemd unit (as trellis-ci) stays up (#219).'
    }
}

Write-Host ''
Write-Host 'done. Start and check:'
Write-Host "  schtasks /run /tn $TaskName"
if (-not $SkipWsl) { Write-Host "  schtasks /run /tn $WslTaskName" }
Write-Host '  gh api repos/maydaythecoder/trellis/actions/runners --jq ".runners[] | [.name, .status] | @tsv"'
Write-Host "  Get-CimInstance Win32_Process -Filter `"Name='Runner.Listener.exe'`" | Invoke-CimMethod -MethodName GetOwner"
