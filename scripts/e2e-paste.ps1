# End-to-end test: Saywell (running with --inject-wav) dictates into a throwaway WinForms text box,
# whose text is read back with UI Automation and compared. Also checks the clipboard was restored.
# It never touches your own apps (Notepad on Windows 11 shares one process across windows).
# Usage: powershell -File scripts/e2e-paste.ps1 [-Exe path] [-Clip tts01] [-HoldMs 600] [-Mode hold|doubletap]
#   hold:      press the hotkey for HoldMs, release.
#   doubletap: tap twice quickly (hands-free), wait HoldMs, tap once more to finish.
#   cancel:    a single short tap (ignored), then hold and press Esc (cancelled). Nothing may be pasted.
param(
  [string]$Exe = "$PSScriptRoot\..\src-tauri\target\debug\saywell.exe",
  [string]$Clip = 'tts01',
  [int]$HoldMs = 600,
  [ValidateSet('hold', 'doubletap', 'cancel')][string]$Mode = 'hold',
  # Optional: save a PNG of the overlay window (only its own rectangle) 1 s before the key is released.
  [string]$Shot = '',
  [switch]$Caption,
  # Optional dictionary for the test profile: comma-separated terms and one "find=>replace" rule.
  [string]$Terms = '',
  [string]$Rule = '',
  # Expected text, if different from the clip's reference (e.g. after a replacement rule).
  [string]$Expect = ''
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Kbd {
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr FindWindow(string cls, string title);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public struct RECT { public int Left, Top, Right, Bottom; }
  // The test copy uses F24 as its dictation key (set below), so the user's own keys never trigger it.
  public static void Hotkey(bool down) { keybd_event(0x87, 0, down ? 0u : 2u, UIntPtr.Zero); }
  public static void Esc() { keybd_event(0x1B, 0x01, 0, UIntPtr.Zero); keybd_event(0x1B, 0x01, 2, UIntPtr.Zero); }
}
'@

$root = Resolve-Path "$PSScriptRoot\.."
$wav = Join-Path $root "tests\fixtures\audio\$Clip.wav"
$expected = (Get-Content (Join-Path $root "tests\fixtures\audio\$Clip.txt") -Raw).Trim([char]0xFEFF, ' ', "`r", "`n")
# Throwaway profile: F24 hotkey, separate settings and log.
$env:APPDATA = Join-Path $env:TEMP "saywell-e2e-profile-$PID"
New-Item -ItemType Directory -Force (Join-Path $env:APPDATA 'Saywell') | Out-Null
Set-Content (Join-Path $env:APPDATA 'Saywell\settings.toml') "hotkey_vk = 135`nhotkey_label = `"F24`"`npreview_caption = $(if ($Caption) { 'true' } else { 'false' })`n"
$log = Join-Path $env:APPDATA 'Saywell\logs\saywell.log'
$db = Join-Path $env:APPDATA 'Saywell\saywell.db'
$dbTool = Join-Path $PSScriptRoot 'e2e_db.py'
if ($Terms -or $Rule) { python -I $dbTool seed $db $Terms $Rule }
if ($Expect) { $expected = $Expect }
if ($Shot) { $env:RUST_LOG = 'info,saywell_lib::ui=debug,saywell_lib::controller=debug' } # logs the overlay position
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
  do { Start-Sleep -Milliseconds 300; $new = NewLog } until (($new -match '] ready' -and $new -match 'cleanup model (ready|failed)|cleanup off') -or (Get-Date) -gt $deadline -or $app.HasExited)
  if ($new -notmatch '\] ready') { throw "Saywell did not become ready. Log:`n$new" }

  $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($targetScript))
  $target = Start-Process powershell -ArgumentList '-NoProfile', '-EncodedCommand', $enc -PassThru -WindowStyle Minimized
  $byName = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $title)
  $el = $null; $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 300; $el = [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, $byName) } until ($el -or (Get-Date) -gt $deadline)
  if (-not $el) { throw 'test window did not appear' }
  [Kbd]::SetForegroundWindow([IntPtr]$el.Current.NativeWindowHandle) | Out-Null
  Start-Sleep -Milliseconds 500

  if ($Mode -eq 'hold') {
    [Kbd]::Hotkey($true)
    try {
      if ($Shot) {
        Start-Sleep -Milliseconds ($HoldMs - 1000)
        $m = [regex]::Match((NewLog), 'overlay at (-?\d+),(-?\d+) size (\d+)x(\d+)')
        if ($m.Success) {
          Add-Type -AssemblyName System.Drawing
          $x, $y, $w, $h = 1..4 | ForEach-Object { [int]$m.Groups[$_].Value }
          $bmp = New-Object Drawing.Bitmap $w, $h
          [Drawing.Graphics]::FromImage($bmp).CopyFromScreen($x, $y, 0, 0, $bmp.Size); $bmp.Save($Shot)
          "overlay  : ${w}x${h} at $x,$y; focus kept: $([Kbd]::GetForegroundWindow() -eq [IntPtr]$el.Current.NativeWindowHandle)"
        } else { 'overlay  : position not in log' }
        Start-Sleep -Milliseconds 1000
      } else { Start-Sleep -Milliseconds $HoldMs }
    } finally { [Kbd]::Hotkey($false) }
  } elseif ($Mode -eq 'cancel') {
    [Kbd]::Hotkey($true); Start-Sleep -Milliseconds 90; [Kbd]::Hotkey($false); Start-Sleep -Milliseconds 900
    [Kbd]::Hotkey($true)
    try { Start-Sleep -Milliseconds 1500; [Kbd]::Esc(); Start-Sleep -Milliseconds 300 } finally { [Kbd]::Hotkey($false) }
    Start-Sleep -Milliseconds 2500
    $new = NewLog
    $got = if (Test-Path $outFile) { "$(Get-Content $outFile -Raw)".Trim() } else { '' }
    "tap ignored: $($new -match 'single short tap')   cancelled: $($new -match 'dictation cancelled')   pasted: '$got'"
    if ($new -match 'single short tap' -and $new -match 'dictation cancelled' -and $new -notmatch 'dictation: ' -and -not $got) { 'E2E PASS'; return } else { 'E2E FAIL'; exit 1 }
  } else {
    [Kbd]::Hotkey($true); Start-Sleep -Milliseconds 90; [Kbd]::Hotkey($false); Start-Sleep -Milliseconds 120
    [Kbd]::Hotkey($true); Start-Sleep -Milliseconds 90; [Kbd]::Hotkey($false)
    Start-Sleep -Milliseconds $HoldMs
    [Kbd]::Hotkey($true); Start-Sleep -Milliseconds 90; [Kbd]::Hotkey($false)
  }
  $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 200; $new = NewLog } until ($new -match 'dictation: |failed|error' -or (Get-Date) -gt $deadline)
  Start-Sleep -Milliseconds 500

  # The test window saves its text every 200 ms; give it up to 3 s before concluding nothing arrived.
  $readUntil = (Get-Date).AddSeconds(3)
  do {
    $got = if (Test-Path $outFile) { "$(Get-Content $outFile -Raw -ErrorAction SilentlyContinue)".Trim() } else { '' }
    if (-not $got) { Start-Sleep -Milliseconds 250 }
  } until ($got -or (Get-Date) -gt $readUntil)
  $clipNow = [System.Windows.Forms.Clipboard]::GetText()
  $timing = ([regex]::Matches($new, 'dictation: .*') | Select-Object -Last 1).Value

  "expected : $expected"
  "pasted   : $got"
  "clipboard: $clipNow"
  "log      : $timing"
  if ($Shot) { "target pid $($target.Id); $(([regex]::Matches($new, 'inserting into foreground pid \d+') | Select-Object -Last 1).Value)" }
  $norm = { param($s) ($s.ToLower() -replace '[^a-z0-9 ]', '' -replace '\s+', ' ').Trim() }
  $ok = $got.Length -gt 0 -and ((& $norm $got) -eq (& $norm $expected) -or $got.Length -ge 0.8 * $expected.Length)
  if ($clipNow -ne 'SAYWELL-SENTINEL') { 'FAIL: clipboard was not restored'; $ok = $false }
  if (Test-Path $db) { python -I $dbTool history $db }
  $previews = [regex]::Matches($new, 'preview: ').Count
  $handsFree = $new -match 'hands-free recording'
  "previews : $previews   hands-free: $handsFree"
  if ($Mode -eq 'doubletap' -and -not $handsFree) { 'FAIL: hands-free mode was not entered'; $ok = $false }
  if ($ok) { 'E2E PASS' } else { 'E2E FAIL'; exit 1 }
}
finally {
  if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -ErrorAction SilentlyContinue }
  if ($target -and -not $target.HasExited) { Stop-Process -Id $target.Id -ErrorAction SilentlyContinue; $target.WaitForExit(3000) | Out-Null }
  Remove-Item $outFile -ErrorAction SilentlyContinue
  # The speech servers exit with Saywell but may hold their log files for a moment.
  if ($app) { $app.WaitForExit(5000) | Out-Null }
  $until = (Get-Date).AddSeconds(8)
  while ((Test-Path $env:APPDATA) -and (Get-Date) -lt $until) {
    Remove-Item -Recurse -Force $env:APPDATA -ErrorAction SilentlyContinue
    if (Test-Path $env:APPDATA) { Start-Sleep -Milliseconds 400 }
  }
}
