# Offline audit: run Saywell (test profile, F24 hotkey, test clip instead of the mic), dictate once,
# and list every TCP/UDP endpoint owned by Saywell and its child processes (whisper-server,
# llama-server, WebView2) while it works. PASS = nothing but loopback.
# Usage: powershell -File scripts/offline-audit.ps1 [-Exe path]
param([string]$Exe = "$PSScriptRoot\..\src-tauri\target\release\saywell.exe")
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, UIAutomationClient, UIAutomationTypes
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class AK {
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  public static void F24(bool down) { keybd_event(0x87, 0, down ? 0u : 2u, UIntPtr.Zero); }
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
}
'@
$root = Resolve-Path "$PSScriptRoot\.."
$env:APPDATA = Join-Path $env:TEMP "saywell-audit-$PID"
New-Item -ItemType Directory -Force (Join-Path $env:APPDATA 'Saywell') | Out-Null
# Release builds don't look in the repo for models: point the profile at them.
$models = (Resolve-Path (Join-Path $root 'models')).Path
Set-Content (Join-Path $env:APPDATA 'Saywell\settings.toml') "models_dir = '$models'`nhotkey_vk = 135`nhotkey_label = `"F24`"`n"
$log = Join-Path $env:APPDATA 'Saywell\logs\saywell.log'
$wav = Join-Path $root 'tests\fixtures\audio\tts04.wav'

function Tree($rootPid) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
  $ids = @($rootPid); $grew = $true
  while ($grew) {
    $more = $all | Where-Object { $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId } | ForEach-Object ProcessId
    $grew = [bool]$more; $ids += $more
  }
  $all | Where-Object { $ids -contains $_.ProcessId }
}
$seen = @{}
# The WebView2 *browser* process (no --type=) belongs to the Windows WebView2 runtime. It can open
# its own Microsoft connections outside Chromium's network stack (browser switches don't apply).
# Saywell's pages can't reach the network (CSP, dead proxy, DNS blackhole), and no audio or text
# passes through that process, so such connections are reported separately, not hidden.
$runtimeProcs = @{}
function Sample($rootPid) {
  $procs = Tree $rootPid
  $names = @{}; $procs | ForEach-Object { $names[[int]$_.ProcessId] = $_.Name }
  Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { [int[]]@($procs.ProcessId) -contains $_.ProcessId -and $_.CommandLine -notmatch '--type=' } | ForEach-Object { $script:runtimeProcs[[int]$_.ProcessId] = $true; $names[[int]$_.ProcessId] = 'msedgewebview2.exe (runtime)' }
  $pids = [int[]]@($procs.ProcessId)
  Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess } | ForEach-Object {
    $k = "TCP $($names[[int]$_.OwningProcess]) $($_.LocalAddress) -> $($_.RemoteAddress) $($_.State)"
    $script:seen[$k] = @($_.LocalAddress, $_.RemoteAddress)
  }
  Get-NetUDPEndpoint -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess } | ForEach-Object {
    $k = "UDP $($names[[int]$_.OwningProcess]) $($_.LocalAddress):$($_.LocalPort)"
    $script:seen[$k] = @($_.LocalAddress, '')
  }
}

$userText = [System.Windows.Forms.Clipboard]::GetText()
$p = Start-Process $Exe -ArgumentList '--inject-wav', "`"$wav`"" -PassThru
try {
  $deadline = (Get-Date).AddSeconds(90)
  do { Start-Sleep -Milliseconds 500; Sample $p.Id; $t = Get-Content $log -Raw -ErrorAction SilentlyContinue } until (($t -match '\] ready' -and $t -match 'cleanup model (ready|failed)') -or (Get-Date) -gt $deadline)
  "ready: $($t -match '\] ready')"
  # Dictate once into a throwaway window of our own, never into the user's apps.
  $title = "Saywell Audit Target $PID"
  $target = Start-Process powershell -ArgumentList '-NoProfile', '-Command', "Add-Type -A System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = '$title'; `$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t); [Windows.Forms.Application]::Run(`$f)" -PassThru
  $cond = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $title)
  $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 300; $w = [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, $cond) } until ($w -or (Get-Date) -gt $deadline)
  if (-not $w) { throw 'audit target window did not appear' }
  [AK]::SetForegroundWindow([IntPtr]$w.Current.NativeWindowHandle) | Out-Null; Start-Sleep -Milliseconds 500
  [AK]::F24($true); try { for ($i = 0; $i -lt 6; $i++) { Start-Sleep -Milliseconds 250; Sample $p.Id } } finally { [AK]::F24($false) }
  for ($i = 0; $i -lt 12; $i++) { Start-Sleep -Milliseconds 250; Sample $p.Id }
  $t = Get-Content $log -Raw
  "dictation ran: $($t -match 'dictation: ')"
  $loop = '^(127\.|::1$|0\.0\.0\.0$|::$)'
  $external = $seen.GetEnumerator() | Where-Object { $_.Value[0] -notmatch $loop -or ($_.Value[1] -and $_.Value[1] -notmatch $loop) }
  $runtime = $external | Where-Object { $_.Key -like '* (runtime) *' }
  $bad = $external | Where-Object { $_.Key -notlike '* (runtime) *' }
  "endpoints seen: $($seen.Count)"
  $seen.Keys | Sort-Object | ForEach-Object { "  $_" }
  if ($runtime) { 'NOTE: WebView2 runtime connections (Windows component, not Saywell traffic):'; $runtime | ForEach-Object { "  $($_.Key)" } }
  if ($bad) { 'NON-LOOPBACK from Saywell or its helpers:'; $bad | ForEach-Object { "  $($_.Key)" }; 'AUDIT FAIL' } else { 'AUDIT PASS: Saywell, whisper-server, llama-server and the page processes use loopback only' }
} finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force; $p.WaitForExit(5000) | Out-Null }
  if ($target -and -not $target.HasExited) { Stop-Process -Id $target.Id -Force }
  if ($userText) { [System.Windows.Forms.Clipboard]::SetText($userText) }
  Start-Sleep 1
  Remove-Item -Recurse -Force $env:APPDATA -ErrorAction SilentlyContinue
}
