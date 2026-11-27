# End-to-end test: Saywell (running with --inject-wav) dictates into a throwaway WinForms text box,
# whose text is read back with UI Automation and compared. Also checks the clipboard was restored.
# It never touches your own apps (Notepad on Windows 11 shares one process across windows).
# Usage: powershell -File scripts/e2e-paste.ps1 [-Exe path] [-Clip tts01]
param(
  [string]$Exe = "$PSScriptRoot\..\src-tauri\target\debug\saywell.exe",
  [string]$Clip = 'tts01'
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Kbd {
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public static void RightCtrl(bool down) { keybd_event(0xA3, 0x1D, 1u | (down ? 0u : 2u), UIntPtr.Zero); }
}
'@

$root = Resolve-Path "$PSScriptRoot\.."
$wav = Join-Path $root "tests\fixtures\audio\$Clip.wav"
$expected = (Get-Content (Join-Path $root "tests\fixtures\audio\$Clip.txt") -Raw).Trim([char]0xFEFF, ' ', "`r", "`n")
$log = Join-Path $env:APPDATA 'Saywell\logs\saywell.log'
$logStart = if (Test-Path $log) { (Get-Item $log).Length } else { 0 }
function NewLog { if (-not (Test-Path $log)) { return '' }; $fs = [IO.File]::Open($log, 'Open', 'Read', 'ReadWrite'); $fs.Seek($logStart, 'Begin') | Out-Null; $t = (New-Object IO.StreamReader($fs)).ReadToEnd(); $fs.Close(); $t }

$title = "Saywell E2E Target $PID"
$outFile = Join-Path $env:TEMP "saywell-e2e-$PID.txt"
Remove-Item $outFile -ErrorAction SilentlyContinue
$targetScript = @"
Add-Type -AssemblyName System.Windows.Forms
`$f = New-Object Windows.Forms.Form; `$f.Text = '$title'; `$f.Width = 700; `$f.Height = 300; `$f.TopMost = `$true
`$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t)
`$tm = New-Object Windows.Forms.Timer; `$tm.Interval = 200; `$tm.Add_Tick({ [IO.File]::WriteAllText('$outFile', `$t.Text) }); `$tm.Start()
`$f.StartPosition = 'CenterScreen'; `$f.Add_Shown({ `$f.WindowState = 'Normal'; `$f.Activate(); `$t.Focus() }); [Windows.Forms.Application]::Run(`$f)
"@

[System.Windows.Forms.Clipboard]::SetText('SAYWELL-SENTINEL')
$app = Start-Process -FilePath $Exe -ArgumentList '--inject-wav', "`"$wav`"" -PassThru
$target = $null
try {
  $deadline = (Get-Date).AddSeconds(90)
  do { Start-Sleep -Milliseconds 300; $new = NewLog } until ($new -match '\] ready' -or (Get-Date) -gt $deadline -or $app.HasExited)
  if ($new -notmatch '\] ready') { throw "Saywell did not become ready. Log:`n$new" }

  $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($targetScript))
  $target = Start-Process powershell -ArgumentList '-NoProfile', '-EncodedCommand', $enc -PassThru -WindowStyle Minimized
  $byName = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $title)
  $el = $null; $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 300; $el = [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, $byName) } until ($el -or (Get-Date) -gt $deadline)
  if (-not $el) { throw 'test window did not appear' }
  [Kbd]::SetForegroundWindow([IntPtr]$el.Current.NativeWindowHandle) | Out-Null
  Start-Sleep -Milliseconds 500

  [Kbd]::RightCtrl($true); Start-Sleep -Milliseconds 600; [Kbd]::RightCtrl($false)
  $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 200; $new = NewLog } until ($new -match 'dictation: |failed|error' -or (Get-Date) -gt $deadline)
  Start-Sleep -Milliseconds 500

  $got = if (Test-Path $outFile) { (Get-Content $outFile -Raw).Trim() } else { '' }
  $clipNow = [System.Windows.Forms.Clipboard]::GetText()
  $timing = ([regex]::Matches($new, 'dictation: .*') | Select-Object -Last 1).Value

  "expected : $expected"
  "pasted   : $got"
  "clipboard: $clipNow"
  "log      : $timing"
  $norm = { param($s) ($s.ToLower() -replace '[^a-z0-9 ]', '' -replace '\s+', ' ').Trim() }
  $ok = $got.Length -gt 0 -and ((& $norm $got) -eq (& $norm $expected) -or $got.Length -ge 0.8 * $expected.Length)
  if ($clipNow -ne 'SAYWELL-SENTINEL') { 'FAIL: clipboard was not restored'; $ok = $false }
  if ($ok) { 'E2E PASS' } else { 'E2E FAIL'; exit 1 }
}
finally {
  Remove-Item $outFile -ErrorAction SilentlyContinue
  if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
  if ($target -and -not $target.HasExited) { Stop-Process -Id $target.Id -ErrorAction SilentlyContinue }
}
