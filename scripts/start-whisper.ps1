# Starts the local whisper.cpp server that Relay uses for speech to text.
# Listens on 127.0.0.1:8080 only. Relay setting: OpenAI-compatible engine,
# base URL http://127.0.0.1:8080/v1. No API key needed.
param(
    [string]$Model = 'ggml-small-q5_1.bin',
    # 12 was fastest on an i5-14400F (2.5 s for 11 s of audio).
    [int]$Threads = 12,
    [int]$Port = 8080
)
$root = Join-Path (Split-Path $PSScriptRoot -Parent) '.whisper'
$server = Join-Path $root 'bin\whisper-server.exe'
$modelPath = Join-Path $root "models\$Model"
if (-not (Test-Path $server)) { throw "whisper-server.exe not found at $server" }
if (-not (Test-Path $modelPath)) { throw "Model not found at $modelPath" }
# --audio-ctx 768 covers 15 s of audio. Relay never sends chunks longer than 12 s,
# so the default 30 s window only adds work. Relay passes the language per request
# when Settings > Spoken language is set; auto detection costs a second pass.
& $server --model $modelPath --host 127.0.0.1 --port $Port --threads $Threads --language auto --audio-ctx 768 --inference-path /v1/audio/transcriptions
