param([Parameter(Mandatory = $true)][string]$AppPath)
$ErrorActionPreference='Stop'
if(Get-Process -Name kynn-voice -ErrorAction SilentlyContinue){throw 'Close Kynn Voice before this isolated test.'}
$app=[System.IO.Path]::GetFullPath($AppPath)
$audit=Join-Path ([System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))) 'target\persistence-audit'
New-Item -ItemType Directory -Path $audit -Force | Out-Null
$config=Join-Path $audit 'configuração de inicialização.toml'
$runPath='Software\Microsoft\Windows\CurrentVersion\Run'
function Read-OwnEntry {
    $key=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runPath,$false)
    try { if($key){return $key.GetValue('Kynn Voice',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)} } finally {if($key){$key.Dispose()}}
}
if($null -ne (Read-OwnEntry)){throw 'NOT_RUN: a Kynn Voice startup entry already exists; preserving it.'}
$expected='"'+$app+'" --config "'+$config+'"'
@'
mode = 'studio'
start_processing = false
clarity_percent = 42
suppression_level = 0.37
'@ | Set-Content -LiteralPath $config -Encoding utf8
# Only the named Kynn Voice Run value is temporarily registered. No shell,
# driver, audio default, StartupApproved value, task or security setting changes.
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class KynnStartupProbe {
 public delegate bool E(IntPtr h,IntPtr p);
 [DllImport("user32.dll")] static extern bool EnumWindows(E e,IntPtr p);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint id);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h,StringBuilder s,int max);
 [DllImport("user32.dll")] static extern IntPtr GetDlgItem(IntPtr h,int id);
 [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h,uint msg,IntPtr w,IntPtr l,uint flags,uint ms,out IntPtr result);
 public static IntPtr Window(int process){IntPtr found=IntPtr.Zero;EnumWindows((h,p)=>{uint id;GetWindowThreadProcessId(h,out id);var s=new StringBuilder(256);GetWindowText(h,s,256);if(id==process && s.ToString()=="Kynn Voice — Configurações" && GetDlgItem(h,124)!=IntPtr.Zero){found=h;return false;}return true;},IntPtr.Zero);return found;}
 public static long Send(IntPtr h,uint msg,int w){IntPtr result;if(h==IntPtr.Zero || SendMessageTimeout(h,msg,(IntPtr)w,IntPtr.Zero,2,2000,out result)==IntPtr.Zero)throw new Exception("Native control unavailable or unresponsive");return result.ToInt64();}
 public static bool Checked(IntPtr p){return Send(GetDlgItem(p,124),0x00F0,0)==1;}
 public static void CheckAndQuit(IntPtr p,bool enabled){if(Checked(p)!=enabled)Send(GetDlgItem(p,124),0x00F5,0);Send(GetDlgItem(p,123),0x00F5,0);}
}
'@
$probe=$null
function Launch([string]$name,[bool]$settings) {
    $argsList=@('--config',('"'+$config+'"'));if($settings){$argsList+= '--settings'}
    $script:probe=Start-Process -FilePath $app -ArgumentList $argsList -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $audit "startup-$name.stdout.log") -RedirectStandardError (Join-Path $audit "startup-$name.stderr.log")
    $deadline=[DateTime]::UtcNow.AddSeconds(15)
    do {
        Start-Sleep -Milliseconds 150
        if($probe.HasExited){throw "App exited: $($probe.ExitCode)"}
        $output=Get-Content -LiteralPath (Join-Path $audit "startup-$name.stdout.log") -Raw
        $script:window=[KynnStartupProbe]::Window($probe.Id)
        $ready=if($settings){$window -ne [IntPtr]::Zero -and $output -match 'Settings opened using native Windows controls'}else{$output -match 'System tray initialized'}
    } while(-not $ready -and [DateTime]::UtcNow -lt $deadline)
    if(-not $ready){throw 'App initialization timed out.'}
}
function Await-Quit {
    if(-not $probe.WaitForExit(5000)){throw 'Quit timed out.'}
    $probe.Refresh();if($probe.ExitCode -ne 0){throw "Quit failed: $($probe.ExitCode)"}
}
try {
    Launch 'enable' $true
    [KynnStartupProbe]::CheckAndQuit($window,$true)
    Await-Quit
    $registered=Read-OwnEntry
    if($registered -cne $expected){throw 'Startup registration did not match the absolute executable/config command after immediate quit.'}
    $registered | Set-Content -LiteralPath (Join-Path $audit 'startup-command.txt')
    Launch 'disable' $true
    $deadline=[DateTime]::UtcNow.AddSeconds(3)
    while(-not [KynnStartupProbe]::Checked($window) -and [DateTime]::UtcNow -lt $deadline){Start-Sleep -Milliseconds 100}
    if(-not [KynnStartupProbe]::Checked($window)){throw 'Startup checkbox did not restore the actual registry state.'}
    [KynnStartupProbe]::CheckAndQuit($window,$false)
    Await-Quit
    if($null -ne (Read-OwnEntry)){throw 'Startup value was not removed after immediate quit.'}
    # Same executable/config arguments as the checked Run command, but no
    # actual logoff/reboot. The file intentionally preserves paused audio.
    Launch 'logon-command' $false
    Start-Sleep -Seconds 1
    $output=Get-Content -LiteralPath (Join-Path $audit 'startup-logon-command.stdout.log') -Raw
    if($output -notmatch 'Restored saved paused audio state' -or $output -match 'Opening settings window|Pipeline started'){throw 'Background launch did not restore the paused profile silently.'}
    Write-Output 'STARTUP_PASS: opt-in UI enable/read-back, immediate quit flush, restored checkbox, disable/read-back, quoted Unicode config, background launch without opening settings; real logon NOT_RUN.'
} finally {
    if($probe -and -not $probe.HasExited){Stop-Process -Id $probe.Id -ErrorAction SilentlyContinue}
    # Roll back only the command owned by this test. Never delete another value
    # if an external application changed it during the test.
    if((Read-OwnEntry) -ceq $expected){
        $key=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runPath,$true)
        try{$key.DeleteValue('Kynn Voice',$false)}finally{$key.Dispose()}
    }
}
