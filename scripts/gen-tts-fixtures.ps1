# Generates offline English test clips with Windows' built-in SAPI voices.
# Output: tests/fixtures/audio/<id>.wav (16 kHz, 16-bit, mono) + <id>.txt reference.
param([string]$Root = (Split-Path -Parent $PSScriptRoot))
Add-Type -AssemblyName System.Speech
$dir = Join-Path $Root 'tests\fixtures\audio'
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$voices = @($synth.GetInstalledVoices() | Where-Object { $_.Enabled -and $_.VoiceInfo.Culture.Name -like 'en-*' } | ForEach-Object { $_.VoiceInfo.Name })
if ($voices.Count -eq 0) { throw 'No English SAPI voices installed' }
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$i = 0
foreach ($line in Get-Content (Join-Path $dir 'sentences.txt')) {
  if (-not $line.Trim()) { continue }
  $id, $text = $line -split '\|', 2
  $synth.SelectVoice($voices[$i % $voices.Count]); $i++
  $synth.SetOutputToWaveFile((Join-Path $dir "$id.wav"), $fmt)
  $synth.Speak($text)
  $synth.SetOutputToNull()
  Set-Content -Path (Join-Path $dir "$id.txt") -Value $text -Encoding UTF8 -NoNewline
}
$synth.Dispose()
"Generated $i clips using voices: $($voices -join ', ')"
