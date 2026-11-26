// Settings: the one page that shows every option. Each feature also carries its own setup
// where it is used (Live, the editor, Notes, Agent); those controls and the ones here edit the
// same settings, and every change is saved as it is made.

import { useEffect, useState, type ReactNode } from 'react';
import * as api from '../api';
import { HotkeyCapture, PageHeader } from '../components';
import { IconRefresh } from '../icons';
import { patchSettings, pushToast, refreshCatalog, reportError, selCatalogBusy, selSettingsSave, useCatalog, useRelay } from '../store';
import type { ApiKeyProvider, InsertMethod, ModelFeature, ModelSource, Settings, WhisperStatus } from '../types';
import { AgentSetup } from './AgentTab';
import { CLEANUP_HINT, CleanupSwitch, INSTRUCTIONS_PLACEHOLDER } from './Editor';
import { NotesSetup } from './LiveSetup';
import {
  DeviceSelect,
  FeatureModel,
  LangChips,
  ModelTest,
  NumField,
  SpeechPicker,
  Toggle,
  useAudioDevices,
  useSettings,
  WhereChip,
} from './setup';

function Field({
  label,
  htmlFor,
  hint,
  children,
}: {
  label: string;
  htmlFor?: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="setting">
      <label className="setting-label" htmlFor={htmlFor}>
        {label}
      </label>
      <div className="setting-control">
        {children}
        {hint ? <div className="hint">{hint}</div> : null}
      </div>
    </div>
  );
}

/** Store, replace or remove one key. Listing the models again shows what the key unlocks. */
function KeyField({ provider, label }: { provider: ApiKeyProvider; label: string }) {
  const [stored, setStored] = useState<boolean | null>(null);
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    try {
      setStored(await api.hasApiKey(provider));
    } catch (e) {
      reportError('has_api_key', e);
    }
  };

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider]);

  const write = async (key: string) => {
    setBusy(true);
    try {
      await api.setApiKey(provider, key);
      setValue('');
      await refresh();
      void refreshCatalog();
    } catch (e) {
      reportError('set_api_key', e);
    } finally {
      setBusy(false);
    }
  };

  const id = `key-${provider}`;
  return (
    <div className="key-field">
      <label className="sr-only" htmlFor={id}>
        {label}
      </label>
      <input
        id={id}
        type="password"
        autoComplete="off"
        spellCheck={false}
        placeholder={stored ? 'Key stored. Paste a new one to replace it' : `Paste the ${label}`}
        value={value}
        onChange={(e) => setValue(e.target.value)}
      />
      <button type="button" className="btn sm" onClick={() => void write(value.trim())} disabled={busy || value.trim() === ''}>
        Save key
      </button>
      {stored ? (
        <button type="button" className="btn ghost sm" onClick={() => void write('')} disabled={busy}>
          Remove
        </button>
      ) : null}
    </div>
  );
}

/** Presets for the local or custom server. */
const SERVER_PRESETS: Array<{ id: string; label: string; url: string }> = [
  { id: 'ollama', label: 'Ollama (this PC)', url: 'http://localhost:11434/v1' },
  { id: 'lmstudio', label: 'LM Studio (this PC)', url: 'http://localhost:1234/v1' },
  { id: 'groq', label: 'Groq', url: 'https://api.groq.com/openai/v1' },
  { id: 'openrouter', label: 'OpenRouter', url: 'https://openrouter.ai/api/v1' },
];
const PRESET_CUSTOM = 'custom';

function presetForUrl(url: string): string {
  const u = url.trim().replace(/\/+$/, '').toLowerCase();
  return SERVER_PRESETS.find((p) => p.url === u)?.id ?? PRESET_CUSTOM;
}

function SourceStatus({ info }: { info: ModelSource | null }) {
  if (!info) return <span className="chip subtle">Checking</span>;
  if (!info.available) return <span className="chip subtle">{(info.note ?? 'Not available').replace(/\.$/, '')}</span>;
  const n = info.models.length;
  return <span className="chip ok">{n === 1 ? '1 model' : `${n} models`}</span>;
}

/** One model source: where it runs, what it offers now, and the key or address it needs. */
function SourceCard({ info, title, children }: { info: ModelSource | null; title: string; children: ReactNode }) {
  return (
    <li className="source">
      <div className="source-head">
        <span className="source-name">{info?.label ?? title}</span>
        {info ? <WhereChip local={info.local} /> : null}
        <span className="spacer" />
        <SourceStatus info={info} />
      </div>
      {children}
      {info && info.available && info.note ? <div className="hint">{info.note}</div> : null}
    </li>
  );
}

function Sources({ settings }: { settings: Settings }) {
  const catalog = useCatalog();
  const busy = useRelay(selCatalogBusy);
  const get = (s: ModelSource['source']) => catalog?.sources.find((x) => x.source === s) ?? null;
  const presetId = presetForUrl(settings.llmBaseUrl);
  return (
    <>
      <div className="source-bar">
        <p className="muted">
          A source appears in every model list once it can run: a cloud source needs its key, a local server needs to be
          running. Keys go to Windows Credential Manager and are never shown again.
        </p>
        <button type="button" className="btn sm" onClick={() => void refreshCatalog()} disabled={busy}>
          <IconRefresh size={13} /> {busy ? 'Checking' : 'Check again'}
        </button>
      </div>
      <ul className="sources">
        <SourceCard info={get('anthropic')} title="Claude">
          <KeyField provider="anthropic" label="Anthropic key" />
        </SourceCard>
        <SourceCard info={get('openAi')} title="OpenAI">
          <KeyField provider="openai" label="OpenAI key" />
          <div className="source-row">
            <label htmlFor="openaiBaseUrl">Address</label>
            <input
              id="openaiBaseUrl"
              type="text"
              spellCheck={false}
              value={settings.openaiBaseUrl}
              onChange={(e) => patchSettings({ openaiBaseUrl: e.target.value })}
            />
          </div>
          <div className="hint">Also used for speech to text with the OpenAI-compatible engine.</div>
        </SourceCard>
        <SourceCard info={get('server')} title="Local server">
          <div className="source-row">
            <label htmlFor="llmPreset">Server</label>
            <select
              id="llmPreset"
              value={presetId}
              onChange={(e) => {
                const p = SERVER_PRESETS.find((x) => x.id === e.target.value);
                patchSettings({ llmBaseUrl: p ? p.url : '' });
              }}
            >
              {SERVER_PRESETS.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
              <option value={PRESET_CUSTOM}>Other address</option>
            </select>
          </div>
          <div className="source-row">
            <label htmlFor="llmBaseUrl">Address</label>
            <input
              id="llmBaseUrl"
              type="text"
              spellCheck={false}
              placeholder="http://localhost:11434/v1"
              value={settings.llmBaseUrl}
              onChange={(e) => patchSettings({ llmBaseUrl: e.target.value })}
            />
          </div>
          <KeyField provider="openai-llm" label="server key (optional)" />
          <div className="hint">
            Any server that speaks the OpenAI chat format. The key is optional for servers on this PC and is only sent
            over https or to this PC.
          </div>
        </SourceCard>
        <li className="source">
          <div className="source-head">
            <span className="source-name">Deepgram</span>
            <WhereChip local={false} />
            <span className="spacer" />
            <span className="chip subtle">Speech to text only</span>
          </div>
          <KeyField provider="deepgram" label="Deepgram key" />
        </li>
      </ul>
    </>
  );
}

const ROUTES: Array<{ feature: ModelFeature; label: string; hint: string }> = [
  { feature: 'translationLlm', label: 'Live translation', hint: 'Once per caption, so speed matters most.' },
  { feature: 'dictationLlm', label: 'Dictation and editing', hint: 'AI cleanup of dictation and of text in the editor.' },
  { feature: 'notesLlm', label: 'Meeting notes', hint: 'Summary, decisions and action items.' },
  { feature: 'plannerLlm', label: 'Task planner', hint: 'Cleans up, splits and merges backlog drafts into goals.' },
];

function fmtMb(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

/** Local Whisper model list: pick a downloaded model, or download one. */
function WhisperModels({ value, onChange }: { value: string; onChange: (name: string) => void }) {
  const [status, setStatus] = useState<WhisperStatus | null>(null);
  const [progress, setProgress] = useState<{ name: string; done: number; total: number } | null>(null);

  const refresh = () =>
    api
      .whisperStatus()
      .then(setStatus)
      .catch((e) => reportError('whisper_status', e));

  useEffect(() => {
    void refresh();
    let unlisten: (() => void) | undefined;
    let live = true;
    void api
      .onWhisperDownload((p) => setProgress(p))
      .then((u) => {
        if (live) unlisten = u;
        else u();
      });
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  const download = async (name: string) => {
    setProgress({ name, done: 0, total: 0 });
    setStatus((s) => (s ? { ...s, downloading: name } : s));
    try {
      await api.downloadWhisperModel(name);
      pushToast('info', 'Model downloaded.', name);
      onChange(name);
      void refreshCatalog();
    } catch (e) {
      reportError('download_whisper_model', e);
    } finally {
      setProgress(null);
      void refresh();
    }
  };

  if (!status) return <div className="muted">Checking local models.</div>;
  if (!status.built)
    return (
      <div className="hint">
        This build has no local Whisper. Rebuild with the cargo feature local-whisper, or choose another engine.
      </div>
    );

  const selected = status.models.find((m) => m.name === value);
  return (
    <div className="stack">
      <ul className="model-list" aria-label="Whisper models">
        {status.models.map((m) => {
          const busy = status.downloading === m.name;
          const pct = busy && progress && progress.total > 0 ? Math.floor((progress.done / progress.total) * 100) : null;
          return (
            <li key={m.name} className={`model-row${m.name === value ? ' on' : ''}`}>
              <label className="check grow">
                <input
                  type="radio"
                  name="whisperModel"
                  checked={m.name === value}
                  disabled={!m.downloaded}
                  onChange={() => onChange(m.name)}
                />
                <span>{m.label}</span>
              </label>
              <span className="muted small tabular">{m.sizeMb} MB</span>
              {m.downloaded ? (
                <span className="chip ok">On this PC</span>
              ) : busy ? (
                <>
                  <span className="muted small tabular" role="status">
                    {pct === null ? 'Starting' : `${pct}% of ${fmtMb(progress?.total ?? 0)}`}
                  </span>
                  <button type="button" className="btn ghost sm" onClick={() => void api.cancelWhisperDownload()}>
                    Cancel
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className="btn sm"
                  disabled={status.downloading !== null}
                  onClick={() => void download(m.name)}
                >
                  Download
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {selected && !selected.downloaded ? (
        <div className="hint">The selected model is not downloaded yet. Download it or pick another.</div>
      ) : null}
      <div className="hint">Models are stored in {status.modelsDir}. Transcription runs on this PC with no network calls.</div>
    </div>
  );
}

const SECTIONS: Array<{ id: string; label: string }> = [
  { id: 's-sources', label: 'Model sources' },
  { id: 's-routes', label: 'Model per feature' },
  { id: 's-stt', label: 'Speech to text' },
  { id: 's-tr', label: 'Translation' },
  { id: 's-audio', label: 'Audio' },
  { id: 's-dict', label: 'Dictation' },
  { id: 's-notes', label: 'Meeting notes' },
  { id: 's-agent', label: 'Agent' },
  { id: 's-keys2', label: 'Hotkeys' },
];

function SettingsIndex() {
  return (
    <nav className="settings-index" aria-label="Settings sections">
      {SECTIONS.map((s) => (
        <button
          key={s.id}
          type="button"
          onClick={() => document.getElementById(s.id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })}
        >
          {s.label}
        </button>
      ))}
    </nav>
  );
}

const SAVE_TEXT = { saved: 'All changes saved', saving: 'Saving', error: 'Not saved' } as const;

export function SettingsTab() {
  const s = useSettings();
  const saveState = useRelay(selSettingsSave);
  const devices = useAudioDevices();

  if (!s)
    return (
      <div className="page">
        <PageHeader title="Settings" subtitle="Loading settings." />
      </div>
    );

  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => patchSettings({ [k]: v } as Pick<Settings, K>);
  const mics = devices.filter((d) => d.kind === 'microphone');
  const outs = devices.filter((d) => d.kind === 'system');

  return (
    <div className="page">
      <PageHeader
        title="Settings"
        subtitle="Every option in one place. Changes are saved as you make them."
        actions={
          <span className={`save-state ${saveState}`} role="status">
            <span className={`dot${saveState === 'saved' ? ' ok' : saveState === 'saving' ? ' warn' : ''}`} aria-hidden="true" />
            {SAVE_TEXT[saveState]}
          </span>
        }
      />
      <div className="settings-layout">
        <SettingsIndex />
        <div className="settings">
          <section className="set-section" aria-labelledby="s-sources">
            <h2 id="s-sources">Model sources</h2>
            <Sources settings={s} />
          </section>

          <section className="set-section" aria-labelledby="s-routes">
            <h2 id="s-routes">Model per feature</h2>
            <p className="muted">
              Each feature takes any model from any source that can run. A local model keeps the text on this PC. The
              same pickers sit next to each feature in Live, the editor, Notes and Agent.
            </p>
            {ROUTES.map((r) => (
              <Field key={r.feature} label={r.label} htmlFor={`route-${r.feature}`} hint={r.hint}>
                <div className="route">
                  <FeatureModel id={`route-${r.feature}`} feature={r.feature} />
                  <ModelTest choice={s[r.feature]} />
                </div>
              </Field>
            ))}
            <p className="hint">The agent model is under Agent. It runs on Claude, because it needs tool use.</p>
          </section>

          <section className="set-section" aria-labelledby="s-stt">
            <h2 id="s-stt">Speech to text</h2>
            <div className="setting-stack">
              <SpeechPicker idPrefix="set" />
            </div>
            {s.sttEngine === 'localWhisper' && (
              <>
                <Field label="Whisper models">
                  <WhisperModels value={s.whisperModel} onChange={(n) => set('whisperModel', n)} />
                </Field>
                <Field
                  label="Custom model file"
                  htmlFor="whisperModelPath"
                  hint="Optional. A ggml file path here overrides the model picked above."
                >
                  <input
                    id="whisperModelPath"
                    type="text"
                    placeholder="Leave empty to use the model picked above"
                    value={s.whisperModelPath}
                    onChange={(e) => set('whisperModelPath', e.target.value)}
                  />
                </Field>
              </>
            )}
            {s.sttEngine === 'openAiCompatible' && (
              <Field label="Model id" htmlFor="openaiSttModel" hint="Uses the OpenAI source above: its address and its key.">
                <input id="openaiSttModel" type="text" value={s.openaiSttModel} onChange={(e) => set('openaiSttModel', e.target.value)} />
              </Field>
            )}
            {s.sttEngine === 'deepgram' && (
              <Field label="Model id" htmlFor="deepgramModel">
                <input id="deepgramModel" type="text" value={s.deepgramModel} onChange={(e) => set('deepgramModel', e.target.value)} />
              </Field>
            )}
            <Field label="Spoken language" htmlFor="sourceLanguage" hint="Leave empty to detect the language.">
              <input
                id="sourceLanguage"
                type="text"
                placeholder="Detect automatically"
                value={s.sourceLanguage ?? ''}
                onChange={(e) => set('sourceLanguage', e.target.value.trim() === '' ? null : e.target.value.trim())}
              />
            </Field>
          </section>

          <section className="set-section" aria-labelledby="s-tr">
            <h2 id="s-tr">Translation</h2>
            <Field label="Translate into" htmlFor="targetLanguages" hint="A code or a name, for example es, fr, Japanese. With no language, translation is off.">
              <LangChips id="targetLanguages" value={s.targetLanguages} onChange={(v) => set('targetLanguages', v)} />
            </Field>
            <Field label="Translate speech from">
              <div className="stack">
                <Toggle id="translateSystem" checked={s.translateSystem} onChange={(b) => set('translateSystem', b)} label="Others (system audio)" />
                <Toggle id="translateMic" checked={s.translateMic} onChange={(b) => set('translateMic', b)} label="You (microphone)" />
              </div>
            </Field>
          </section>

          <section className="set-section" aria-labelledby="s-audio">
            <h2 id="s-audio">Audio</h2>
            <Field label="Microphone">
              <Toggle id="captureMic" checked={s.captureMic} onChange={(b) => set('captureMic', b)} label="Capture microphone" />
            </Field>
            <Field label="Microphone device" htmlFor="micDevice">
              <DeviceSelect id="micDevice" value={s.micDevice} devices={mics} onChange={(v) => set('micDevice', v)} />
            </Field>
            <Field label="System audio">
              <Toggle id="captureSystem" checked={s.captureSystem} onChange={(b) => set('captureSystem', b)} label="Capture system audio" />
            </Field>
            <Field label="System audio device" htmlFor="systemDevice" hint="An output device, captured by loopback.">
              <DeviceSelect id="systemDevice" value={s.systemDevice} devices={outs} onChange={(v) => set('systemDevice', v)} />
            </Field>
            <Field label="Voice detection threshold" htmlFor="vadThreshold" hint="RMS level between 0 and 1. Lower values pick up quieter speech.">
              <NumField id="vadThreshold" value={s.vadThreshold} min={0} max={1} step={0.001} onChange={(n) => set('vadThreshold', n)} />
            </Field>
            <Field label="Silence to end a chunk (ms)" htmlFor="silenceMs">
              <NumField id="silenceMs" value={s.silenceMs} min={100} step={50} integer onChange={(n) => set('silenceMs', n)} />
            </Field>
            <Field label="Longest chunk (ms)" htmlFor="maxChunkMs">
              <NumField id="maxChunkMs" value={s.maxChunkMs} min={1000} step={500} integer onChange={(n) => set('maxChunkMs', n)} />
            </Field>
            <Field
              label="Keep recordings"
              hint="Saves the audio of each dictation and live session with its History entry, on this PC only. Deleting the entry deletes the audio."
            >
              <Toggle id="saveAudio" checked={s.saveAudio} onChange={(b) => set('saveAudio', b)} label="Keep recordings in History" />
            </Field>
          </section>

          <section className="set-section" aria-labelledby="s-dict">
            <h2 id="s-dict">Dictation and editing</h2>
            <p className="muted">
              Defaults for the editor steps. Dictation uses them when it stops; the editor on the Live tab starts from them.
            </p>
            <Field label="Cleanup" hint={CLEANUP_HINT[s.cleanup]}>
              <CleanupSwitch name="set-cleanup" value={s.cleanup} onChange={(m) => set('cleanup', m)} />
            </Field>
            <Field label="Instructions" htmlFor="instructions" hint="Free-form. Used by AI cleanup only.">
              <textarea
                id="instructions"
                rows={4}
                placeholder={INSTRUCTIONS_PLACEHOLDER}
                value={s.instructions}
                onChange={(e) => set('instructions', e.target.value)}
              />
            </Field>
            <Field label="Match the app">
              <Toggle
                id="autoToneFromApp"
                checked={s.autoToneFromApp}
                onChange={(b) => set('autoToneFromApp', b)}
                label="Adjust tone to the app you dictate into (email, chat, code)"
              />
            </Field>
            <Field label="About you" htmlFor="userProfile" hint="Your role or vocabulary. Sent with polish requests.">
              <textarea id="userProfile" rows={3} value={s.userProfile} onChange={(e) => set('userProfile', e.target.value)} />
            </Field>
            <Field label="Insert method" htmlFor="insertMethod">
              <select id="insertMethod" value={s.insertMethod} onChange={(e) => set('insertMethod', e.target.value as InsertMethod)}>
                <option value="paste">Paste with the clipboard</option>
                <option value="type">Type keystrokes</option>
              </select>
            </Field>
            <Field label="Auto insert">
              <Toggle id="autoInsertDictation" checked={s.autoInsertDictation} onChange={(b) => set('autoInsertDictation', b)} label="Insert dictation when it stops" />
            </Field>
          </section>

          <section className="set-section" aria-labelledby="s-notes">
            <h2 id="s-notes">Meeting notes</h2>
            <div className="setting-stack">
              <NotesSetup idPrefix="set" />
            </div>
          </section>

          <section className="set-section" aria-labelledby="s-agent">
            <h2 id="s-agent">Agent</h2>
            <div className="setting-stack">
              <AgentSetup idPrefix="set" />
            </div>
          </section>

          <section className="set-section" aria-labelledby="s-keys2">
            <h2 id="s-keys2">Hotkeys</h2>
            <Field label="Start or stop Live" htmlFor="hotkeyToggle" hint="Opens the Live tab and records in the last mode you picked.">
              <HotkeyCapture
                id="hotkeyToggle"
                value={s.hotkeyToggle}
                defaultValue="CommandOrControl+Shift+Space"
                otherValue={s.hotkeyDictate}
                onChange={(v) => set('hotkeyToggle', v)}
              />
            </Field>
            <Field label="Start or stop dictation" htmlFor="hotkeyDictate" hint="Works in any app. Needs Ctrl or Alt.">
              <HotkeyCapture
                id="hotkeyDictate"
                value={s.hotkeyDictate}
                defaultValue="CommandOrControl+Shift+D"
                otherValue={s.hotkeyToggle}
                onChange={(v) => set('hotkeyDictate', v)}
              />
            </Field>
          </section>
        </div>
      </div>
    </div>
  );
}
