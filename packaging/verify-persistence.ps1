param(
    [Parameter(Mandatory = $true)][string]$AppPath,
    [switch]$ExpectLegacyFailure
)
$ErrorActionPreference = 'Stop'
if (Get-Process -Name kynn-voice -ErrorAction SilentlyContinue) { throw 'Close the running Kynn Voice before this isolated test.' }
$app = [System.IO.Path]::GetFullPath($AppPath)
$audit = Join-Path ([System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))) 'target\persistence-audit'
New-Item -ItemType Directory -Path $audit -Force | Out-Null
$tag = if ($ExpectLegacyFailure) { 'baseline' } else { 'fixed' }
$config = Join-Path $audit "$tag-config.toml"
@'
input_device = 'Kynn Voice persistence test input (unavailable)'
mode = 'light'
start_processing = false
denoise_enabled = true
suppression_level = 0.75
'@ | Set-Content -LiteralPath $config -Encoding utf8
# Messages below target only the process created by this script. No Windows
# defaults, real startup entries, user config, drivers or audio files are changed.
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class KynnPersistenceProbe {
 public delegate bool EnumProc(IntPtr h,IntPtr p);
 [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p,IntPtr arg);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h,StringBuilder s,int size);
 [DllImport("user32.dll")] static extern IntPtr GetDlgItem(IntPtr h,int id);
 [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h,uint msg,IntPtr w,IntPtr l,uint flags,uint timeout,out IntPtr result);
 public static IntPtr Window(int process) {IntPtr found=IntPtr.Zero; EnumWindows((h,p)=>{uint pid;GetWindowThreadProcessId(h,out pid); var text=new StringBuilder(256);GetWindowText(h,text,256); if(pid==process && text.ToString()=="Kynn Voice — Configurações" && GetDlgItem(h,103)!=IntPtr.Zero){found=h;return false;}return true;},IntPtr.Zero);return found;}
 public static long Send(IntPtr h,uint msg,long w,long l){IntPtr result;if(h==IntPtr.Zero || SendMessageTimeout(h,msg,(IntPtr)w,(IntPtr)l,2,2000,out result)==IntPtr.Zero)throw new Exception("Native control unavailable or unresponsive");return result.ToInt64();}
 public static long Get(IntPtr p,int id,uint msg){return Send(GetDlgItem(p,id),msg,0,0);}
 public static void Select(IntPtr p,int id,int value){Send(GetDlgItem(p,id),0x014E,value,0);Send(p,0x0111,(1<<16)|id,0);}
 public static void Slider(IntPtr p,int id,int value){IntPtr child=GetDlgItem(p,id);Send(child,0x0405,1,value);Send(p,0x0114,0,child.ToInt64());}
 public static void Check(IntPtr p,int id,bool value){if((Get(p,id,0x00F0)==1)!=value)Send(GetDlgItem(p,id),0x00F5,0,0);}
 public static void Click(IntPtr p,int id){Send(GetDlgItem(p,id),0x00F5,0,0);}
}
'@
function Start-Probe([string]$name) {
    $script:probe = Start-Process -FilePath $app -ArgumentList @('--settings','--config',('"'+$config+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $audit "$tag-$name.stdout.log") -RedirectStandardError (Join-Path $audit "$tag-$name.stderr.log")
    $deadline=[DateTime]::UtcNow.AddSeconds(15)
    do {
        Start-Sleep -Milliseconds 150
        if($probe.HasExited){throw "App exited before opening: $($probe.ExitCode)"}
        $script:window=[KynnPersistenceProbe]::Window($probe.Id)
        $ready=(Get-Content -LiteralPath (Join-Path $audit "$tag-$name.stdout.log") -Raw) -match 'Settings opened using native Windows controls'
    } while(($window -eq [IntPtr]::Zero -or -not $ready) -and [DateTime]::UtcNow -lt $deadline)
    if(-not $ready){throw 'Native window initialization timed out.'}
}
$probe=$null
try {
    Start-Probe 'change'
    [KynnPersistenceProbe]::Select($window,103,2)
    [KynnPersistenceProbe]::Slider($window,107,37)
    [KynnPersistenceProbe]::Slider($window,116,155)
    [KynnPersistenceProbe]::Slider($window,117,65)
    [KynnPersistenceProbe]::Slider($window,109,73)
    [KynnPersistenceProbe]::Slider($window,111,42)
    [KynnPersistenceProbe]::Slider($window,112,-37)
    [KynnPersistenceProbe]::Slider($window,113,28)
    [KynnPersistenceProbe]::Slider($window,114,19)
    [KynnPersistenceProbe]::Check($window,106,$false)
    [KynnPersistenceProbe]::Check($window,108,$true)
    [KynnPersistenceProbe]::Check($window,110,$true)
    [KynnPersistenceProbe]::Check($window,115,$true)
    if(-not $ExpectLegacyFailure){
        $deadline=[DateTime]::UtcNow.AddSeconds(8)
        while([KynnPersistenceProbe]::Get($window,101,0x0146) -lt 2 -and [DateTime]::UtcNow -lt $deadline){Start-Sleep -Milliseconds 150}
        if([KynnPersistenceProbe]::Get($window,101,0x0146) -lt 2){throw 'No physical microphone available for device-selection persistence test.'}
        [KynnPersistenceProbe]::Select($window,101,1)
        [KynnPersistenceProbe]::Click($window,104)
    }
    Start-Sleep -Seconds 3
    $saved = Get-Content -LiteralPath $config -Raw
    $saved | Set-Content -LiteralPath (Join-Path $audit "$tag-before-forced-exit.toml")
    $savedLive = $saved -match 'mode\s*=\s*"studio"'
    # Deliberate abrupt termination of this test-owned process, to exercise
    # recovery without relying on the normal quit handler.
    Stop-Process -Id $probe.Id
    $probe.WaitForExit()
    Start-Probe 'restore'
    $mode=[KynnPersistenceProbe]::Get($window,103,0x0147)
    $suppression=[KynnPersistenceProbe]::Get($window,107,0x0400)
    if($ExpectLegacyFailure){
        if($savedLive -or $mode -ne 0 -or $suppression -ne 75){throw 'The old binary no longer exhibits the expected baseline failure.'}
        Write-Output 'BASELINE_REPRODUCED: live choices were not saved; forced exit restored old mode/light and reduction/75.'
    } else {
        if(-not $savedLive -or $mode -ne 2){throw 'Mode did not autosave/restore.'}
        foreach($item in @(@(107,37),@(116,155),@(117,65),@(109,73),@(111,42),@(112,-37),@(113,28),@(114,19))){
            $actual=[KynnPersistenceProbe]::Get($window,$item[0],0x0400)
            if($actual -ne $item[1]){throw "Control $($item[0]) restored $actual instead of $($item[1])."}
        }
        foreach($item in @(@(106,0),@(108,1),@(110,1),@(115,1))){if([KynnPersistenceProbe]::Get($window,$item[0],0x00F0) -ne $item[1]){throw "Checkbox $($item[0]) was not restored."}}
        if($saved -notmatch 'input_device\s*=\s*"wasapi:' -or $saved -notmatch 'start_processing\s*=\s*false'){throw 'Manual endpoint ID or paused state was not persisted.'}
        $restoredLog=Get-Content -LiteralPath (Join-Path $audit "$tag-restore.stdout.log") -Raw
        if($restoredLog -match 'Pipeline started'){throw 'Restart opened audio despite saved paused state.'}
        [KynnPersistenceProbe]::Click($window,123)
        if(-not $probe.WaitForExit(5000)){throw 'Normal quit did not finish.'}
        if($probe.ExitCode -ne 0){throw 'Normal quit reported an error.'}
        Write-Output 'PERSISTENCE_PASS: live autosave, forced exit/restart, mode/bypass/gains/gate/EQ/clarity/autogain/device ID restored; paused state retained without audio.'
    }
} finally {if($probe -and -not $probe.HasExited){Stop-Process -Id $probe.Id -ErrorAction SilentlyContinue}}
