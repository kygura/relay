// Setup for live transcription and translation, next to the transcript. Everything here is
// saved as it changes. Model choices apply to the next caption; speech, language and capture
// choices apply when the next recording starts, because a recording keeps the setup it began with.

import { patchSettings, useCatalog } from '../store';
import type { Mode, Settings } from '../types';
import {
  DeviceSelect,
  FeatureModel,
  LangChips,
  modelName,
  NumField,
  SetupGroup,
  SetupRow,
  SpeechPicker,
  speechOf,
  Toggle,
  useAudioDevices,
  useSettings,
  WhereChip,
  sourceOf,
} from './setup';

type SessionMode = Exclude<Mode, 'idle'>;

/** Notes controls, shared with the Notes page. */
export function NotesSetup({ idPrefix }: { idPrefix: string }) {
  const s = useSettings();
  if (!s) return null;
  return (
    <>
      <SetupRow label="Notes model" htmlFor={`${idPrefix}-notes-model`}>
        <FeatureModel id={`${idPrefix}-notes-model`} feature="notesLlm" />
      </SetupRow>
      <SetupRow label="Refresh every (seconds)" htmlFor={`${idPrefix}-notes-int`} hint="At least 30 seconds.">
        <NumField
          id={`${idPrefix}-notes-int`}
          value={s.notesIntervalSecs}
          min={30}
          step={10}
          integer
          onChange={(n) => patchSettings({ notesIntervalSecs: n })}
        />
      </SetupRow>
      <SetupRow label="Action items" hint="New action items land in the Agent backlog as drafts. Nothing runs until you start it.">
        <Toggle
          id={`${idPrefix}-auto-tasks`}
          checked={s.autoExtractTasks}
          onChange={(b) => patchSettings({ autoExtractTasks: b })}
          label="Add action items to the backlog"
        />
      </SetupRow>
    </>
  );
}

export function LiveSetup({ mode, recording }: { mode: SessionMode; recording: boolean }) {
  const s = useSettings();
  const catalog = useCatalog();
  const devices = useAudioDevices();
  if (!s) return <p className="muted setup-loading">Loading setup.</p>;

  const speech = speechOf(catalog, s.sttEngine);
  const trSource = sourceOf(catalog, s.translationLlm.source);
  const translates = mode === 'translate' || mode === 'meeting';
  const mics = devices.filter((d) => d.kind === 'microphone');
  const outs = devices.filter((d) => d.kind === 'system');
  const noTargets = s.targetLanguages.length === 0;
  const noSource = !s.translateMic && !s.translateSystem;

  return (
    <div className="setup-scroll" tabIndex={-1}>
      {recording ? (
        <p className="setup-note" role="status">
          Recording. Model changes apply to the next caption. Speech, language and capture changes apply to the next
          recording.
        </p>
      ) : null}

      <SetupGroup title="Speech" aside={speech ? <WhereChip local={speech.local} /> : null}>
        <SpeechPicker idPrefix="ls" />
        <SetupRow label="Spoken language" htmlFor="ls-source-lang" hint="Leave empty to detect it.">
          <input
            id="ls-source-lang"
            type="text"
            placeholder="Detect automatically"
            spellCheck={false}
            value={s.sourceLanguage ?? ''}
            onChange={(e) => patchSettings({ sourceLanguage: e.target.value.trim() === '' ? null : e.target.value.trim() })}
          />
        </SetupRow>
      </SetupGroup>

      <SetupGroup title="Translation" aside={trSource ? <WhereChip local={trSource.local} /> : null}>
        {!translates ? <p className="hint">Used in Translate and Meeting modes.</p> : null}
        <SetupRow
          label="Translate into"
          htmlFor="ls-targets"
          hint={noTargets ? <span className="warn">No target language. Translation is off.</span> : 'Each caption is translated into every language here.'}
        >
          <LangChips id="ls-targets" value={s.targetLanguages} onChange={(v) => patchSettings({ targetLanguages: v })} />
        </SetupRow>
        <SetupRow
          label="Translate speech from"
          hint={noSource && !noTargets ? <span className="warn">Pick at least one source to translate.</span> : undefined}
        >
          <div className="setup-toggles">
            <Toggle id="ls-tr-system" checked={s.translateSystem} onChange={(b) => patchSettings({ translateSystem: b })} label="Others (system audio)" />
            <Toggle id="ls-tr-mic" checked={s.translateMic} onChange={(b) => patchSettings({ translateMic: b })} label="You (microphone)" />
          </div>
        </SetupRow>
        <SetupRow label="Translation model" htmlFor="ls-tr-model" hint="Runs once per caption. A small, fast model keeps the text close to the speech.">
          <FeatureModel id="ls-tr-model" feature="translationLlm" />
        </SetupRow>
      </SetupGroup>

      <SetupGroup title="Capture">
        <SetupRow label="Microphone">
          <Toggle id="ls-cap-mic" checked={s.captureMic} onChange={(b) => patchSettings({ captureMic: b })} label="Capture your microphone" />
          {s.captureMic ? (
            <DeviceSelect id="ls-mic-dev" label="Microphone device" value={s.micDevice} devices={mics} onChange={(v) => patchSettings({ micDevice: v })} />
          ) : null}
        </SetupRow>
        <SetupRow label="System audio" hint="The other side of a call, captured from an output device.">
          <Toggle id="ls-cap-sys" checked={s.captureSystem} onChange={(b) => patchSettings({ captureSystem: b })} label="Capture system audio" />
          {s.captureSystem ? (
            <DeviceSelect id="ls-sys-dev" label="System audio device" value={s.systemDevice} devices={outs} onChange={(v) => patchSettings({ systemDevice: v })} />
          ) : null}
        </SetupRow>
      </SetupGroup>

      <SetupGroup title="Meeting notes">
        {mode !== 'meeting' ? <p className="hint">Notes refresh on their own in Meeting mode.</p> : null}
        <NotesSetup idPrefix="ls" />
      </SetupGroup>
    </div>
  );
}

/** One line that says what a recording in this mode will use. Opens the setup panel. */
export function setupSummary(s: Settings, catalog: ReturnType<typeof useCatalog>, mode: SessionMode): string[] {
  const speech = speechOf(catalog, s.sttEngine);
  const sttModel = s.sttEngine === 'localWhisper' ? s.whisperModel : s.sttEngine === 'deepgram' ? s.deepgramModel : s.openaiSttModel;
  const out = [`${speech?.label ?? 'Speech'}: ${sttModel}`];
  if (mode === 'translate' || mode === 'meeting') {
    const from = s.sourceLanguage ? s.sourceLanguage.toUpperCase() : 'Auto';
    out.push(
      s.targetLanguages.length === 0
        ? 'No target language'
        : `${from} to ${s.targetLanguages.map((l) => l.toUpperCase()).join(', ')}: ${modelName(catalog, s.translationLlm)}`,
    );
  }
  if (mode === 'meeting') out.push(`Notes: ${modelName(catalog, s.notesLlm)}`);
  if (mode === 'dictation') out.push(s.cleanup === 'ai' ? `Cleanup: ${modelName(catalog, s.dictationLlm)}` : `Cleanup: ${s.cleanup}`);
  if (mode === 'agent') out.push(`Agent: ${modelName(catalog, { source: 'anthropic', model: s.agentModel })}`);
  return out;
}
