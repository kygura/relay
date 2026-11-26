// Setup controls shared by every surface that configures a feature in place: the Live setup
// panel, the editor, Notes, Agent and the Settings page. They all read and write the one
// settings copy in the store, so a change made in one place shows in the others.

import { useEffect, useId, useState, type ReactNode } from 'react';
import * as api from '../api';
import { IconClose } from '../icons';
import { patchSettings, selSettings, useCatalog, useRelay } from '../store';
import type {
  AudioDeviceInfo,
  LlmSource,
  ModelCatalog,
  ModelChoice,
  ModelFeature,
  ModelSource,
  Settings,
  SttEngineKind,
} from '../types';

/** The editor fires this before it takes focus, so the Live tab can bring it back into view. */
export const SHOW_EDITOR_EVENT = 'relay:show-editor';

export function useSettings(): Settings | null {
  return useRelay(selSettings);
}

/** One labelled row of a setup panel. The label sits above the control. */
export function SetupRow({
  label,
  htmlFor,
  hint,
  children,
}: {
  label: string;
  htmlFor?: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="setup-row">
      {htmlFor ? (
        <label className="setup-label" htmlFor={htmlFor}>
          {label}
        </label>
      ) : (
        <div className="setup-label">{label}</div>
      )}
      {children}
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** A titled group of setup rows. `aside` sits at the end of the title line. */
export function SetupGroup({ title, aside, children }: { title: string; aside?: ReactNode; children: ReactNode }) {
  const id = useId();
  return (
    <section className="setup-group" aria-labelledby={id}>
      <div className="setup-head">
        <h3 id={id}>{title}</h3>
        {aside}
      </div>
      {children}
    </section>
  );
}

export function Toggle({
  id,
  checked,
  onChange,
  label,
  disabled,
}: {
  id: string;
  checked: boolean;
  onChange: (b: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <label className="check" htmlFor={id}>
      <input id={id} type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
    </label>
  );
}

/** "On this PC" or "Cloud", so it is clear where the text goes. */
export function WhereChip({ local }: { local: boolean }) {
  return <span className={`chip where${local ? ' local' : ''}`}>{local ? 'On this PC' : 'Cloud'}</span>;
}

const SOURCE_FALLBACK: Record<LlmSource, string> = {
  anthropic: 'Claude',
  openAi: 'OpenAI',
  server: 'Local server',
};

export function sourceOf(catalog: ModelCatalog | null, source: LlmSource): ModelSource | null {
  return catalog?.sources.find((s) => s.source === source) ?? null;
}

export function sourceLabel(catalog: ModelCatalog | null, source: LlmSource): string {
  return sourceOf(catalog, source)?.label ?? SOURCE_FALLBACK[source];
}

/** Short name of a chosen model, for summaries: the catalog label when known, else the id. */
export function modelName(catalog: ModelCatalog | null, choice: ModelChoice): string {
  const hit = sourceOf(catalog, choice.source)?.models.find((m) => m.id === choice.model);
  return hit?.label ?? (choice.model || 'No model');
}

const SEP = '\u0001';
const CUSTOM = '__custom__';
const SOURCES: LlmSource[] = ['anthropic', 'openAi', 'server'];

/**
 * Pick a source and a model for one feature. The list comes from the catalog: Claude models
 * when an Anthropic key is stored, the OpenAI account's models when that key is stored, and
 * whatever the local server reports. A source that cannot run yet is listed but disabled, with
 * the reason. "Other model id" takes any id for any source.
 */
export function ModelPicker({
  id,
  value,
  onChange,
  only,
}: {
  id: string;
  value: ModelChoice;
  onChange: (c: ModelChoice) => void;
  /** Limit the list to one source (the agent runs on Claude only). */
  only?: LlmSource;
}) {
  const catalog = useCatalog();
  const sources = SOURCES.filter((s) => !only || s === only);
  const listed = sourceOf(catalog, value.source)?.models.some((m) => m.id === value.model) ?? false;
  const [custom, setCustom] = useState(false);
  // An id the catalog does not list still needs a way to be shown and edited.
  const showCustom = custom || (catalog !== null && !listed);
  const src = sourceOf(catalog, value.source);
  const problem = src && !src.available ? (src.note ?? 'Not available.') : null;

  const pick = (raw: string) => {
    if (raw === CUSTOM) {
      setCustom(true);
      return;
    }
    const [source, model] = raw.split(SEP) as [LlmSource, string];
    setCustom(false);
    onChange({ source, model });
  };

  return (
    <div className="model-picker">
      <select id={id} value={showCustom ? CUSTOM : `${value.source}${SEP}${value.model}`} onChange={(e) => pick(e.target.value)}>
        {catalog === null ? (
          <option value={`${value.source}${SEP}${value.model}`}>{value.model || 'Loading models'}</option>
        ) : null}
        {sources.map((s) => {
          const info = sourceOf(catalog, s);
          if (!info) return null;
          const where = info.local ? 'on this PC' : 'cloud';
          const state = info.available ? where : (info.note ?? 'not available').replace(/\.$/, '');
          return (
            <optgroup key={s} label={`${info.label} (${state})`}>
              {info.models.length === 0 ? (
                <option disabled value={`${s}${SEP}`}>
                  No models listed
                </option>
              ) : null}
              {info.models.map((m) => (
                <option key={m.id} value={`${s}${SEP}${m.id}`} disabled={!info.available}>
                  {m.label === m.id ? m.id : `${m.label}`}
                </option>
              ))}
            </optgroup>
          );
        })}
        <option value={CUSTOM}>Other model id</option>
      </select>
      {showCustom ? (
        <div className="model-custom">
          {only ? null : (
            <select
              aria-label="Source"
              value={value.source}
              onChange={(e) => onChange({ source: e.target.value as LlmSource, model: value.model })}
            >
              {sources.map((s) => (
                <option key={s} value={s}>
                  {sourceLabel(catalog, s)}
                </option>
              ))}
            </select>
          )}
          <input
            type="text"
            aria-label="Model id"
            placeholder="Model id"
            spellCheck={false}
            value={value.model}
            onChange={(e) => onChange({ source: value.source, model: e.target.value.trim() })}
          />
        </div>
      ) : null}
      {problem ? (
        <div className="hint warn" role="status">
          {problem}
        </div>
      ) : null}
    </div>
  );
}

/** Model picker bound to one feature's setting. */
export function FeatureModel({ id, feature }: { id: string; feature: ModelFeature }) {
  const settings = useSettings();
  if (!settings) return null;
  return <ModelPicker id={id} value={settings[feature]} onChange={(c) => patchSettings({ [feature]: c })} />;
}

/** Test button for one model choice, with the result next to it. */
export function ModelTest({ choice }: { choice: ModelChoice }) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const sig = `${choice.source}|${choice.model}`;
  useEffect(() => setResult(null), [sig]);

  const run = async () => {
    setBusy(true);
    setResult(null);
    try {
      setResult({ ok: true, text: await api.testLlm(choice) });
    } catch (e) {
      setResult({ ok: false, text: typeof e === 'string' ? e : e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="model-test">
      <button type="button" className="btn sm" onClick={run} disabled={busy || choice.model.trim() === ''}>
        {busy ? 'Testing' : 'Test'}
      </button>
      {result ? (
        <span className={`test-result${result.ok ? ' ok' : ''}`} role="status" title={result.text}>
          {result.text}
        </span>
      ) : null}
    </div>
  );
}

const COMMON_LANGS: Array<[string, string]> = [
  ['en', 'English'],
  ['es', 'Spanish'],
  ['fr', 'French'],
  ['de', 'German'],
  ['it', 'Italian'],
  ['pt', 'Portuguese'],
  ['nl', 'Dutch'],
  ['pl', 'Polish'],
  ['tr', 'Turkish'],
  ['ru', 'Russian'],
  ['uk', 'Ukrainian'],
  ['ar', 'Arabic'],
  ['hi', 'Hindi'],
  ['zh', 'Chinese'],
  ['ja', 'Japanese'],
  ['ko', 'Korean'],
];

export function langName(code: string): string {
  const hit = COMMON_LANGS.find(([c]) => c === code.toLowerCase());
  return hit ? hit[1] : code;
}

/** Target languages as removable pills with a field to add one. Takes a code or a name. */
export function LangChips({ id, value, onChange }: { id: string; value: string[]; onChange: (v: string[]) => void }) {
  const [text, setText] = useState('');
  const listId = useId();

  const add = (raw: string) => {
    const parts = raw
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean)
      .map((s) => COMMON_LANGS.find(([, n]) => n.toLowerCase() === s.toLowerCase())?.[0] ?? s);
    const next = [...value];
    for (const p of parts) if (!next.some((x) => x.toLowerCase() === p.toLowerCase())) next.push(p);
    if (next.length !== value.length) onChange(next);
    setText('');
  };

  return (
    <div className="lang-chips">
      {value.map((l) => (
        <span key={l} className="chip lang-chip">
          <span className="lang-code">{l}</span>
          {langName(l) !== l ? <span>{langName(l)}</span> : null}
          <button type="button" aria-label={`Remove ${langName(l)}`} onClick={() => onChange(value.filter((x) => x !== l))}>
            <IconClose size={11} />
          </button>
        </span>
      ))}
      <input
        id={id}
        type="text"
        list={listId}
        className="lang-add"
        placeholder={value.length === 0 ? 'Add a language, for example es' : 'Add a language'}
        spellCheck={false}
        value={text}
        onChange={(e) => {
          const v = e.target.value;
          // Picking from the list replaces the text in one step: add it right away.
          const picked = (e.nativeEvent as InputEvent).inputType === 'insertReplacementText';
          if (picked && COMMON_LANGS.some(([c]) => c === v)) add(v);
          else setText(v);
        }}
        onKeyDown={(e) => {
          if ((e.key === 'Enter' || e.key === ',') && text.trim() !== '') {
            e.preventDefault();
            add(text);
          } else if (e.key === 'Backspace' && text === '' && value.length > 0) {
            onChange(value.slice(0, -1));
          }
        }}
        onBlur={() => {
          if (text.trim() !== '') add(text);
        }}
      />
      <datalist id={listId}>
        {COMMON_LANGS.filter(([c]) => !value.includes(c)).map(([c, n]) => (
          <option key={c} value={c}>
            {n}
          </option>
        ))}
      </datalist>
    </div>
  );
}

export function NumField({
  id,
  value,
  onChange,
  min,
  max,
  step,
  integer,
}: {
  id: string;
  value: number;
  onChange: (n: number) => void;
  min?: number;
  max?: number;
  step?: number;
  integer?: boolean;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => {
    setText((t) => (Number(t) === value ? t : String(value)));
  }, [value]);
  const n = Number(text);
  const bad = text.trim() === '' || Number.isNaN(n) || (min !== undefined && n < min) || (max !== undefined && n > max);
  return (
    <input
      id={id}
      type="number"
      inputMode="decimal"
      value={text}
      min={min}
      max={max}
      step={step}
      aria-invalid={bad}
      onChange={(e) => {
        setText(e.target.value);
        const v = Number(e.target.value);
        // Out-of-range values stay in the field, marked invalid, and are not saved.
        const inRange = (min === undefined || v >= min) && (max === undefined || v <= max);
        if (e.target.value.trim() !== '' && !Number.isNaN(v) && inRange) onChange(integer ? Math.round(v) : v);
      }}
    />
  );
}

let devicesOnce: Promise<AudioDeviceInfo[]> | null = null;

/** Audio devices, listed once per window. */
export function useAudioDevices(): AudioDeviceInfo[] {
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([]);
  useEffect(() => {
    let live = true;
    devicesOnce ??= api.listAudioDevices();
    devicesOnce
      .then((d) => live && setDevices(d))
      .catch(() => {
        devicesOnce = null;
      });
    return () => {
      live = false;
    };
  }, []);
  return devices;
}

export function DeviceSelect({
  id,
  value,
  devices,
  onChange,
  label,
}: {
  id: string;
  value: string | null;
  devices: AudioDeviceInfo[];
  onChange: (v: string | null) => void;
  label?: string;
}) {
  const known = value === null || devices.some((d) => d.id === value);
  return (
    <select id={id} aria-label={label} value={value ?? ''} onChange={(e) => onChange(e.target.value === '' ? null : e.target.value)}>
      <option value="">System default</option>
      {!known && value ? <option value={value}>{value} (not found)</option> : null}
      {devices.map((d) => (
        <option key={d.id} value={d.id}>
          {d.name}
          {d.isDefault ? ' (default)' : ''}
        </option>
      ))}
    </select>
  );
}

const STT_FALLBACK: Record<SttEngineKind, string> = {
  localWhisper: 'Local Whisper',
  openAiCompatible: 'OpenAI-compatible server',
  deepgram: 'Deepgram',
};
const ENGINES: SttEngineKind[] = ['localWhisper', 'openAiCompatible', 'deepgram'];

/** The setting that holds the model of each speech engine. */
const STT_MODEL_KEY: Record<SttEngineKind, 'whisperModel' | 'openaiSttModel' | 'deepgramModel'> = {
  localWhisper: 'whisperModel',
  openAiCompatible: 'openaiSttModel',
  deepgram: 'deepgramModel',
};

export function speechOf(catalog: ModelCatalog | null, engine: SttEngineKind) {
  return catalog?.speech.find((s) => s.engine === engine) ?? null;
}

/** Engine and model for speech to text, from the catalog. Engines that cannot run say why. */
export function SpeechPicker({ idPrefix }: { idPrefix: string }) {
  const settings = useSettings();
  const catalog = useCatalog();
  if (!settings) return null;
  const engine = settings.sttEngine;
  const opt = speechOf(catalog, engine);
  const key = STT_MODEL_KEY[engine];
  const model = settings[key];
  const models = opt?.models ?? [];
  const listed = models.some((m) => m.id === model);
  return (
    <>
      <SetupRow label="Engine" htmlFor={`${idPrefix}-engine`} hint={opt && !opt.available ? <span className="warn">{opt.note}</span> : undefined}>
        <select id={`${idPrefix}-engine`} value={engine} onChange={(e) => patchSettings({ sttEngine: e.target.value as SttEngineKind })}>
          {ENGINES.map((k) => {
            const o = speechOf(catalog, k);
            const label = o?.label ?? STT_FALLBACK[k];
            const where = o ? (o.local ? 'on this PC' : 'cloud') : '';
            const tail = o && !o.available ? (o.note ?? 'not available').replace(/\.$/, '') : where;
            return (
              <option key={k} value={k}>
                {tail ? `${label} (${tail})` : label}
              </option>
            );
          })}
        </select>
      </SetupRow>
      <SetupRow label="Speech model" htmlFor={`${idPrefix}-stt-model`}>
        <select id={`${idPrefix}-stt-model`} value={model} onChange={(e) => patchSettings({ [key]: e.target.value })}>
          {!listed ? <option value={model}>{model || 'No model'}</option> : null}
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label}
            </option>
          ))}
        </select>
      </SetupRow>
    </>
  );
}
