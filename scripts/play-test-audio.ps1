# Plays the synthetic test speech through the default output device so Relay can
# capture it by system audio loopback. Repeats with pauses between plays.
param([int]$Times = 12, [int]$PauseSeconds = 4)
$wav = Join-Path (Split-Path $PSScriptRoot -Parent) '.whisper\test-speech.wav'
if (-not (Test-Path $wav)) { throw "Test clip not found at $wav" }
$player = New-Object System.Media.SoundPlayer $wav
for ($i = 1; $i -le $Times; $i++) {
    Write-Host ("Play {0} of {1}" -f $i, $Times)
    $player.PlaySync()
    Start-Sleep -Seconds $PauseSeconds
}
Write-Host 'Done'
