# Relay

Relay is a Windows desktop assistant that listens to your microphone and to system audio (meetings, calls, streams). It transcribes and edits speech, translates live captions, writes meeting notes, and runs a task agent that works on your PC behind permission gates.

Built with Tauri 2: a Rust backend in `src-tauri/` and a React + TypeScript UI in `src/`. Speech to text runs on your PC with whisper.cpp by default. Language model calls go to the Claude Messages API, with a model chosen per task in Settings > Models.

## What it does

| Mode | Input | Output |
|---|---|---|
| Transcribe (default) | Mic and/or system audio | Live captions only. No language model calls, so it works fully offline. |
| Dictate | Microphone | On stop, the text is cleaned up (recognition errors, fillers, spoken self-corrections), matched to the tone of the app you were in, and pasted into that app. |
| Translate | Mic and/or system audio | Live captions with translations into one or more target languages, in the overlay and the Live tab. |
| Meeting | Mic and system audio | Transcript, translations, and meeting notes (summary, decisions, action items, open questions) refreshed every 90 s. Action items can become agent tasks. |
| Agent | Microphone | Each spoken request becomes a task for the agent. |

Exports: TXT, Markdown, SRT, VTT and JSON, with or without translations. Saved files go to `Documents\Relay`.

### Windows

- **Live hotkey**: `Ctrl+Shift+Space` opens the Live tab and starts recording in the last mode you picked there (Transcribe the first time). Press it again to stop.
- **Dictation hotkey**: `Ctrl+Shift+D` starts dictation. Press it again to stop, polish and insert. Both hotkeys can be changed in Settings.
- **Main window**: Live, Notes, Agent and Settings tabs. Closing it keeps Relay running in the tray.
- **Overlay**: a frameless, always-on-top bar with mode buttons, record button, level meters, the last two caption lines and a pending-approval badge. Open it from the tray menu.

### Keyboard

Every action has a shortcut. Press `Ctrl+/` (or `F1`) in Relay for the full list, or `Ctrl+K` for the command palette, which also opens recent History entries. The table lives in `src/keymap.ts`, and every hint in the UI reads from it.

| Keys | Action |
|---|---|
| `Ctrl+Shift+D` (global) | Dictate into any app. Tap to start and stop, or hold to talk and release to finish. A second tap within 0.5 s cancels. |
| `Escape` during dictation | Cancel while listening; skip AI cleanup (use Basic) while cleaning |
| `Ctrl+Shift+Space` (global) | Open Live and start or stop recording |
| `Ctrl+1` to `Ctrl+5` | Live, History, Notes, Agent, Settings |
| `Alt+1` to `Alt+5` | Mode: Transcribe, Dictate, Translate, Meeting, Agent |
| `Ctrl+Enter`, `Ctrl+Shift+Enter`, `Ctrl+Shift+C` | Editor: run, insert, copy |
| `Alt+O`, `Alt+B`, `Alt+A` | Editor cleanup: Off, Basic, AI |
| `Ctrl+E` | Send the live transcript to the editor |
| Up, Down, `Enter`, `Delete`, `Ctrl+Z` | History: move, open in editor, delete, undo |
| `F6` | Move between regions |

While a dictation runs, the overlay shows a small pill with its state: listening, transcribing, cleaning, then "Inserted into {app}". It does not take focus from the app you are typing in.

### Models per task

Settings > Models has one selector per task. Defaults: dictation polish and live translation use Haiku 5.5 for speed, meeting notes use Sonnet 5.5, the agent uses Opus 5.5. A task can also follow the default model, or use any other model id.

### Agent permission levels

Every agent tool has a level. Tools at or below the auto-approve level you set (default: Read only) run without asking. Anything above it pauses the task and shows an approval card.

| Level | Examples | Behaviour |
|---|---|---|
| Read only | active window, clipboard read, screenshot, read file, transcript | Runs automatically by default |
| Local reversible | clipboard write, files inside `Documents\Relay`, open a URL | Asks unless you raise the auto level |
| External | type text, press keys, mouse click, run PowerShell | Asks unless you raise the auto level |
| Destructive | delete a file, write outside `Documents\Relay` | Always asks, cannot be remembered |

The agent stops and asks you (an escalation) when it calls `ask_user`, before anything involving money, credentials, sending messages or deleting data, after 3 tool errors in a row, on a model refusal, or when it reaches 40 steps. "Approve and remember" covers that tool for the rest of the task only. Screen text, files and web pages are treated as data, never as instructions.

## Speech-to-text engines

Choose the engine in Settings.

1. **Local Whisper** (default). whisper.cpp runs inside Relay through whisper-rs 0.16 (the same engine VoxFlow uses). No key, no network, audio stays on your PC. Pick and download a model in Settings > Speech to text; files are checked against a pinned size and SHA-256 and stored in `%APPDATA%\dev.relay.assistant\models`. The default is `small-q5_1` (190 MB, multilingual). Measured on the i5-14400F with Spoken language `en`: first caption 0.6 s after the speaker pauses, 11 s of speech transcribed in 2.1 s.
2. **OpenAI-compatible**. Posts each utterance to `{base URL}/audio/transcriptions`: OpenAI (`gpt-4o-transcribe`), Groq, or a local whisper.cpp or faster-whisper server. Cloud endpoints need a key.
3. **Deepgram**. Streaming websocket with interim results and speaker labels (`nova-3`). Needs a Deepgram key.

Settings files written before local Whisper became the default are migrated once: a cloud engine without a stored key switches to Local Whisper.

## API keys

Enter keys in Settings > API keys. They are stored in Windows Credential Manager under `relay-assistant`, never in the settings file. Environment variables `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` and `DEEPGRAM_API_KEY` are used as a fallback.

## Build and run

Requirements (all installed on this machine during setup): Rust stable (MSVC), Visual Studio 2022 Build Tools with the C++ workload, Node.js LTS, WebView2, CMake and LLVM (libclang, for whisper.cpp). Tool paths for this machine are in `src-tauri\.cargo\config.toml`.

```powershell
cd relay
npm install
npm.cmd run tauri -- dev --features local-whisper
```

`tauri dev` passes `--no-default-features` to cargo, so name the feature. The first build compiles whisper.cpp (a few minutes); it is built optimized even in dev. For a cloud-only binary without CMake or LLVM, build with `--no-default-features`.

Release installer (NSIS and MSI, written to `src-tauri\target\release\bundle`):

```powershell
npm run tauri build
```

Tests (88 unit tests: VAD, resampling, SSE parsing, request bodies, task store, permission gate, export formats, settings migration, model catalog):

```powershell
cd src-tauri; cargo test --lib
```

Preview the UI in a browser with a mock backend: `npm run dev`, then open `http://localhost:1420` (add `?window=overlay` for the overlay).

## Layout

```
src/                    React UI (overlay + main window, typed API in api.ts, mock backend in mock.ts)
src-tauri/src/
  audio/                WASAPI mic and loopback capture, resampling to 16 kHz, energy VAD
  stt/                  OpenAI-compatible, Deepgram and Whisper engines
  llm/                  Claude Messages API client (streaming, tool use, prompt caching, retries)
  agent/                task store (SQLite), tool loop, permission gate, escalation
  os/                   active window, clipboard, text insertion, keys, mouse, screenshots, PowerShell
  pipeline/             polish, translate, meeting notes, export formats
  session.rs            audio -> STT -> per-mode pipelines
  commands.rs, lib.rs   Tauri commands, tray, hotkeys
SPEC.md                 module contracts
```

Data: settings in `%APPDATA%\dev.relay.assistant\settings.json`, tasks in `%APPDATA%\dev.relay.assistant\relay.db`.

## Known limits

- Live system-audio captions with local Whisper are verified. Features that call Claude (dictation polish, translation, notes, agent) are untested live because the Anthropic account used during setup had no credit.
- The Deepgram engine merges mic and system audio into one stream, so speaker labels apply across both sources.
- Mouse clicks assume the screenshot was taken on the primary monitor.
