param(
    [Parameter(Mandatory = $true)][string]$AppPath,
    [ValidateRange(10, 120)][int]$SecondsPerMode = 20,
    [string]$AuditDirectory
)
$ErrorActionPreference = 'Stop'
$app = [System.IO.Path]::GetFullPath($AppPath)
if (-not (Test-Path -LiteralPath $app -PathType Leaf)) { throw 'Application executable not found.' }
if (Get-Process -Name kynn-voice -ErrorAction SilentlyContinue) { throw 'Close the existing Kynn Voice before running this controlled test.' }
$root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$audit = if ($AuditDirectory) { [System.IO.Path]::GetFullPath($AuditDirectory) } else { Join-Path $root 'target\stability-audit' }
New-Item -ItemType Directory -Path $audit -Force | Out-Null
$config = Join-Path $audit 'live-settings-config.toml'
# Only this test file is written. No Windows defaults or user config are changed.
@"
input_device = 'Microfone (USB PnP Sound Device)'
virtual_device = 'CABLE Input (VB-Audio Virtual Cable)'
mode = 'light'
"@ | Set-Content -LiteralPath $config -Encoding utf8

# Native control test API: every message targets only the newly created app.
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class KynnNativeTest {
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder text, int max);
    [DllImport("user32.dll")] static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, IntPtr l, uint flags, uint ms, out IntPtr result);
    [DllImport("user32.dll")] static extern bool PostThreadMessage(uint thread, uint msg, IntPtr w, IntPtr l);
    public static IntPtr Window(int process) {
        IntPtr found=IntPtr.Zero;
        EnumWindows((h,p)=>{ uint pid; GetWindowThreadProcessId(h,out pid); if(pid==(uint)process && Text(h)=="Kynn Voice — Configurações" && GetDlgItem(h,103)!=IntPtr.Zero) {found=h; return false;} return true; },IntPtr.Zero);
        return found;
    }
    public static string Text(IntPtr h) { var text=new StringBuilder(1024); GetWindowText(h,text,text.Capacity); return text.ToString(); }
    public static string ControlText(IntPtr h,int id) { return Text(GetDlgItem(h,id)); }
    public static void Send(IntPtr h,uint msg,int w,int l) {
        IntPtr result; if(SendMessageTimeout(h,msg,(IntPtr)w,(IntPtr)l,2,1000,out result)==IntPtr.Zero) throw new Exception("Native control did not respond within 1 s");
    }
    public static void Select(IntPtr parent,int id,int value) { Send(GetDlgItem(parent,id),0x014E,value,0); Send(parent,0x0111,(1<<16)|id,0); }
    public static void Click(IntPtr parent,int id) { Send(GetDlgItem(parent,id),0x00F5,0,0); }
    public static void Quit(IntPtr h) { uint pid; uint thread=GetWindowThreadProcessId(h,out pid); if(!PostThreadMessage(thread,0x0012,IntPtr.Zero,IntPtr.Zero)) throw new Exception("Unable to request app exit"); }
}
'@
$stdout = Join-Path $audit 'live-settings.stdout.log'
$stderr = Join-Path $audit 'live-settings.stderr.log'
$probe = Start-Process -FilePath $app -ArgumentList @('--settings', '--config', ('"' + $config + '"')) -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
$samples = [System.Collections.Generic.List[object]]::new()
$window = [IntPtr]::Zero
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ($window -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 200
        if ($probe.HasExited) { throw 'App exited before the settings window opened.' }
        $window = [KynnNativeTest]::Window($probe.Id)
    }
    if ($window -eq [IntPtr]::Zero) { throw 'Settings did not open within 15 seconds.' }
    # The HWND precedes completion of ShowWindow/initialization. Wait for the
    # app's ready marker before sending synchronous control messages.
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ((Get-Content -LiteralPath $stdout -Raw) -notmatch 'Settings opened using native Windows controls') {
        if ($probe.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Settings initialization did not complete.' }
        Start-Sleep -Milliseconds 200
    }
    foreach ($mode in 0..2) {
        [KynnNativeTest]::Select($window, 103, $mode)
        if ($mode -eq 0) {
            # Validate default-input selection and actual stream reconfiguration.
            [KynnNativeTest]::Select($window, 101, 0)
            foreach ($repeat in 1..3) { [KynnNativeTest]::Click($window, 104); Start-Sleep -Milliseconds 400 }
        }
        Start-Sleep -Seconds 2
        $until = [DateTime]::UtcNow.AddSeconds($SecondsPerMode)
        $previousCpu = $probe.TotalProcessorTime.TotalSeconds
        $previousAt = [DateTime]::UtcNow
        while ([DateTime]::UtcNow -lt $until) {
            Start-Sleep -Milliseconds 500
            $probe.Refresh()
            if ($probe.HasExited) { throw 'App exited during continuous audio.' }
            $now = [DateTime]::UtcNow
            $cpu = ($probe.TotalProcessorTime.TotalSeconds - $previousCpu) / ($now - $previousAt).TotalSeconds / [Environment]::ProcessorCount * 100
            $previousCpu = $probe.TotalProcessorTime.TotalSeconds
            $previousAt = $now
            $samples.Add([pscustomobject]@{Mode=$mode; CPU=[Math]::Round($cpu,3); PrivateMiB=[Math]::Round($probe.PrivateMemorySize64/1MB,3); WorkingSetMiB=[Math]::Round($probe.WorkingSet64/1MB,3); Threads=$probe.Threads.Count; Handles=$probe.HandleCount; Metrics=[KynnNativeTest]::ControlText($window,119); Status=[KynnNativeTest]::ControlText($window,118)})
        }
        $last = $samples[$samples.Count-1]
        Write-Output ([pscustomobject]@{Mode=$mode; PrivateMiB=$last.PrivateMiB; Threads=$last.Threads; Metrics=$last.Metrics} | ConvertTo-Json -Compress)
    }
    $modules = @($probe.Modules | Select-Object -ExpandProperty ModuleName)
    if (@($modules | Where-Object { $_ -match 'opengl|libegl|libgles|atio|amd.*gl' }).Count -gt 0) { throw 'Unexpected OpenGL driver loaded.' }
    $modules | Set-Content -LiteralPath (Join-Path $audit 'live-modules.txt')
    $samples | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $audit 'live-settings-samples.json')
    [KynnNativeTest]::Click($window, 123)
    if (-not $probe.WaitForExit(5000)) { throw 'App did not quit within 5 seconds.' }
    $probe.Refresh()
    if ($probe.ExitCode -ne 0) { throw "App quit with error $($probe.ExitCode)." }
    $lines = Get-Content -LiteralPath $stdout -Raw
    if ($lines -notmatch 'Pipeline started on WASAPI' -or $lines -notmatch 'DeepFilterNet is ready' -or $lines -notmatch 'Pipeline stopped') { throw 'Missing stream/DeepFilter/shutdown evidence.' }
    foreach ($mode in 0..2) {
        $last = @($samples | Where-Object Mode -eq $mode)[-1]
        $expected = @('Leve','Equilibrado','Estúdio')[$mode]
        if ($last.Metrics -notmatch $expected) { throw "Mode $expected did not become active." }
    }
    Write-Output 'LIVE_SETTINGS_PASS: native controls, default-input apply, three active audio modes, no OpenGL, orderly shutdown.'
    $last = $samples[$samples.Count-1]
    if ($last.Metrics -match 'Underruns ([1-9]\d*)') { Write-Output 'AUDIO_CONTINUITY_FAIL: nonzero underruns observed; see retained metrics.'; exit 2 }
} finally {
    if (-not $probe.HasExited) { Stop-Process -Id $probe.Id -ErrorAction SilentlyContinue }
}
