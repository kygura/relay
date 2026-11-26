//! Settings, persistence and API key storage. FROZEN: owned by the planner.
//! Settings live in `{app_config_dir}/settings.json`. API keys live in Windows
//! Credential Manager through the `keyring` crate, never in the JSON file.

use crate::types::{CleanupMode, InsertMethod, Mode, PermissionLevel};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

const KEYRING_SERVICE: &str = "relay-assistant";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SttEngineKind {
    LocalWhisper,
    OpenAiCompatible,
    Deepgram,
}

/// Where a text feature sends its requests. The agent always uses Claude because it
/// needs tool use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LlmSource {
    /// Claude, with the `anthropic` key.
    #[default]
    Anthropic,
    /// An OpenAI account: `openai_base_url` and the `openai` key.
    OpenAi,
    /// A custom or local OpenAI-compatible server: `llm_base_url` and the optional `openai-llm` key.
    Server,
}

/// The source and model one feature uses.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelChoice {
    pub source: LlmSource,
    pub model: String,
}

impl ModelChoice {
    fn anthropic(model: &str) -> Self {
        Self { source: LlmSource::Anthropic, model: model.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Schema version, used for one-time migrations in `load`.
    pub settings_version: u32,

    // Language models: each text feature picks its own source and model.
    pub dictation_llm: ModelChoice,
    pub translation_llm: ModelChoice,
    pub notes_llm: ModelChoice,
    pub planner_llm: ModelChoice,
    /// Address of the Server source, for example http://localhost:11434/v1.
    pub llm_base_url: String,
    /// Claude model of the agent loop. It stays on Claude because it needs tool use.
    pub agent_model: String,

    // Speech to text
    pub stt_engine: SttEngineKind,
    /// Catalog name of the local Whisper model, see `stt::models::CATALOG`.
    pub whisper_model: String,
    /// Optional custom ggml file. Overrides `whisper_model` when set.
    pub whisper_model_path: String,
    pub openai_base_url: String,
    pub openai_stt_model: String,
    pub deepgram_model: String,
    /// None = auto-detect.
    pub source_language: Option<String>,

    // Translation
    pub target_languages: Vec<String>,
    pub translate_mic: bool,
    pub translate_system: bool,

    // Audio
    pub capture_mic: bool,
    pub mic_device: Option<String>,
    pub capture_system: bool,
    pub system_device: Option<String>,
    pub vad_threshold: f32,
    pub silence_ms: u64,
    pub max_chunk_ms: u64,
    /// Keep the audio of each dictation and live session with its History entry.
    pub save_audio: bool,

    // Dictation and editing: the transform step shared by dictation, edits and transcripts.
    pub cleanup: CleanupMode,
    /// Free-form default instructions for the transform, for example "British spelling".
    pub instructions: String,
    /// Add a tone hint from the app the text goes to.
    pub auto_tone_from_app: bool,
    pub user_profile: String,
    pub insert_method: InsertMethod,
    pub auto_insert_dictation: bool,

    // Meeting notes
    pub notes_interval_secs: u64,
    pub auto_extract_tasks: bool,

    // Agent
    pub agent_auto_level: PermissionLevel,
    pub agent_max_concurrent: usize,

    // Hotkeys
    pub hotkey_toggle: String,
    pub hotkey_dictate: String,

    /// Mode the Live hotkey starts: the last non-dictation mode used.
    pub live_mode: Mode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: SETTINGS_VERSION,
            dictation_llm: ModelChoice::anthropic("claude-haiku-5-5"),
            translation_llm: ModelChoice::anthropic("claude-haiku-5-5"),
            notes_llm: ModelChoice::anthropic("claude-sonnet-5-5"),
            planner_llm: ModelChoice::anthropic("claude-sonnet-5-5"),
            llm_base_url: crate::llm::openai::DEFAULT_BASE_URL.into(),
            agent_model: "claude-opus-5-5".into(),
            stt_engine: SttEngineKind::LocalWhisper,
            whisper_model: crate::stt::models::DEFAULT_MODEL.into(),
            whisper_model_path: String::new(),
            openai_base_url: "https://api.openai.com/v1".into(),
            openai_stt_model: "gpt-4o-transcribe".into(),
            deepgram_model: "nova-3".into(),
            source_language: None,
            target_languages: vec!["es".into()],
            translate_mic: false,
            translate_system: true,
            capture_mic: true,
            mic_device: None,
            capture_system: true,
            system_device: None,
            vad_threshold: 0.012,
            silence_ms: 700,
            max_chunk_ms: 12_000,
            save_audio: true,
            cleanup: CleanupMode::Ai,
            instructions: String::new(),
            auto_tone_from_app: true,
            user_profile: String::new(),
            insert_method: InsertMethod::Paste,
            auto_insert_dictation: true,
            notes_interval_secs: 90,
            auto_extract_tasks: false,
            agent_auto_level: PermissionLevel::ReadOnly,
            agent_max_concurrent: 2,
            hotkey_toggle: "CommandOrControl+Shift+Space".into(),
            hotkey_dictate: "CommandOrControl+Shift+D".into(),
            live_mode: Mode::Transcribe,
        }
    }
}

pub type SharedSettings = Arc<RwLock<Settings>>;

pub const SETTINGS_VERSION: u32 = 4;

/// Which language model a task uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmTask {
    Dictation,
    Translation,
    Notes,
    Planner,
    Agent,
}

impl Settings {
    /// Source and model for a task. The agent is always on Claude.
    pub fn choice_for(&self, task: LlmTask) -> ModelChoice {
        let c = match task {
            LlmTask::Dictation => &self.dictation_llm,
            LlmTask::Translation => &self.translation_llm,
            LlmTask::Notes => &self.notes_llm,
            LlmTask::Planner => &self.planner_llm,
            LlmTask::Agent => {
                let m = self.agent_model.trim();
                let model = if m.is_empty() { crate::llm::DEFAULT_MODEL } else { m };
                return ModelChoice::anthropic(model);
            }
        };
        ModelChoice { source: c.source, model: c.model.trim().to_string() }
    }
}

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

pub fn load(config_dir: &Path) -> Settings {
    let path = settings_path(config_dir);
    let mut s = match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => Settings::default(),
    };
    migrate(&mut s, |p| get_api_key(p).is_some());
    s
}

/// Parse a settings file. Missing fields take their defaults, so older files load unchanged.
fn parse(text: &str) -> Settings {
    // Files written before versioning have no settings_version field.
    let mut v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    if v.get("settingsVersion").is_none() {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("settingsVersion".into(), 1.into());
        }
    }
    migrate_llm_keys(&mut v);
    serde_json::from_value(v).unwrap_or_else(|e| {
        tracing::warn!("settings.json unreadable ({e}), using defaults");
        Settings::default()
    })
}

/// Version 4 replaced `llmProvider`, `llmModel` and the per-task model names with one
/// source and model per feature. Works on the raw JSON, before the typed parse.
fn migrate_llm_keys(v: &mut serde_json::Value) {
    const NEW_KEYS: [&str; 4] = ["dictationLlm", "translationLlm", "notesLlm", "plannerLlm"];
    const OLD_KEYS: [&str; 5] = ["llmProvider", "llmModel", "dictationModel", "translationModel", "notesModel"];
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    let version = obj.get("settingsVersion").and_then(|x| x.as_u64()).unwrap_or(1);
    if version >= 4 || NEW_KEYS.iter().any(|k| obj.contains_key(*k)) {
        return;
    }
    let text = |obj: &serde_json::Map<String, serde_json::Value>, key: &str| -> Option<String> {
        obj.get(key)
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let source = if obj.get("llmProvider").and_then(|x| x.as_str()) == Some("openAiCompatible") {
        LlmSource::Server
    } else {
        LlmSource::Anthropic
    };
    let fallback = text(obj, "llmModel");
    let defaults = Settings::default();
    let pick = |obj: &serde_json::Map<String, serde_json::Value>, key: &str, default: &ModelChoice| ModelChoice {
        source,
        model: text(obj, key).or_else(|| fallback.clone()).unwrap_or_else(|| default.model.clone()),
    };
    let dictation = pick(obj, "dictationModel", &defaults.dictation_llm);
    let translation = pick(obj, "translationModel", &defaults.translation_llm);
    let notes = pick(obj, "notesModel", &defaults.notes_llm);
    for (key, choice) in [
        ("dictationLlm", &dictation),
        ("translationLlm", &translation),
        ("notesLlm", &notes),
        ("plannerLlm", &notes),
    ] {
        obj.insert(key.into(), serde_json::to_value(choice).unwrap_or_default());
    }
    for k in OLD_KEYS {
        obj.remove(k);
    }
}

/// Version 1 defaulted to a cloud STT engine, which fails without a key.
/// Move to local Whisper unless a key exists for the cloud engine in use.
fn migrate(s: &mut Settings, has_key: impl Fn(&str) -> bool) {
    if s.settings_version < 2 {
        let keep_cloud = match s.stt_engine {
            SttEngineKind::LocalWhisper => false,
            SttEngineKind::OpenAiCompatible => has_key("openai"),
            SttEngineKind::Deepgram => has_key("deepgram"),
        };
        if !keep_cloud {
            s.stt_engine = SttEngineKind::LocalWhisper;
        }
        s.settings_version = 2;
    }
    // Version 3 added llm_provider, llm_base_url and live_mode. Older files get their
    // defaults from serde, and every existing value is kept.
    if s.settings_version < 3 {
        s.settings_version = 3;
    }
    // Version 4 moved the model fields to per-feature choices. `parse` already converted
    // the old keys, so only the version number changes here.
    if s.settings_version < 4 {
        s.settings_version = 4;
    }
}

pub fn save(config_dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    std::fs::create_dir_all(config_dir).context("create config dir")?;
    let path = settings_path(config_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Providers: "anthropic", "openai", "deepgram", "openai-llm".
pub fn set_api_key(provider: &str, key: &str) -> anyhow::Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider)?;
    if key.is_empty() {
        let _ = entry.delete_credential();
        return Ok(());
    }
    entry.set_password(key).context("store key in Credential Manager")?;
    Ok(())
}

/// Returns None when no key is stored. Environment variable fallback:
/// ANTHROPIC_API_KEY, OPENAI_API_KEY, DEEPGRAM_API_KEY, OPENAI_LLM_API_KEY.
pub fn get_api_key(provider: &str) -> Option<String> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, provider) {
        if let Ok(k) = entry.get_password() {
            if !k.is_empty() {
                return Some(k);
            }
        }
    }
    let var = format!("{}_API_KEY", provider.to_uppercase().replace('-', "_"));
    std::env::var(var).ok().filter(|k| !k.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_local_whisper() {
        let s = Settings::default();
        assert_eq!(s.stt_engine, SttEngineKind::LocalWhisper);
        assert_eq!(s.settings_version, SETTINGS_VERSION);
    }

    #[test]
    fn v1_cloud_engine_without_key_moves_to_local() {
        let mut s = Settings { settings_version: 1, stt_engine: SttEngineKind::OpenAiCompatible, ..Default::default() };
        migrate(&mut s, |_| false);
        assert_eq!(s.stt_engine, SttEngineKind::LocalWhisper);
        assert_eq!(s.settings_version, SETTINGS_VERSION);
    }

    #[test]
    fn v2_file_gets_new_defaults_and_keeps_values() {
        let text = r#"{"settingsVersion":2,"llmModel":"my-model","dictationModel":"x","sttEngine":"deepgram","targetLanguages":["fr","de"],"hotkeyDictate":"Alt+D"}"#;
        let mut s = parse(text);
        assert_eq!(s.settings_version, 2);
        migrate(&mut s, |_| false);
        assert_eq!(s.settings_version, 4);
        assert_eq!(s.llm_base_url, "http://localhost:11434/v1");
        assert_eq!(s.live_mode, Mode::Transcribe);
        assert_eq!(s.dictation_llm, ModelChoice { source: LlmSource::Anthropic, model: "x".into() });
        assert_eq!(s.translation_llm.model, "my-model");
        assert_eq!(s.notes_llm.model, "my-model");
        assert_eq!(s.stt_engine, SttEngineKind::Deepgram);
        assert_eq!(s.target_languages, vec!["fr".to_string(), "de".to_string()]);
        assert_eq!(s.hotkey_dictate, "Alt+D");
    }

    #[test]
    fn v4_fields_round_trip_in_camel_case() {
        let s = Settings {
            planner_llm: ModelChoice { source: LlmSource::OpenAi, model: "gpt-x".into() },
            notes_llm: ModelChoice { source: LlmSource::Server, model: "llama3.2".into() },
            live_mode: Mode::Meeting,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"plannerLlm\":{\"source\":\"openAi\",\"model\":\"gpt-x\"}"));
        assert!(json.contains("\"notesLlm\":{\"source\":\"server\",\"model\":\"llama3.2\"}"));
        assert!(json.contains("\"dictationLlm\""));
        assert!(json.contains("\"translationLlm\""));
        assert!(json.contains("\"liveMode\":\"meeting\""));
        assert!(!json.contains("llmProvider"));
        let mut back = parse(&json);
        migrate(&mut back, |_| false);
        assert_eq!(back.planner_llm, s.planner_llm);
        assert_eq!(back.notes_llm, s.notes_llm);
        assert_eq!(back.settings_version, 4);
        assert_eq!(back.live_mode, Mode::Meeting);
    }

    #[test]
    fn v3_file_migrates_with_values_kept() {
        let text = r#"{"settingsVersion":3,"llmProvider":"anthropic","llmModel":"claude-sonnet-5-5","dictationModel":"claude-haiku-5-5","translationModel":"","notesModel":"claude-opus-5-5","agentModel":"claude-opus-5-5"}"#;
        let mut s = parse(text);
        migrate(&mut s, |_| false);
        assert_eq!(s.settings_version, 4);
        assert_eq!(s.dictation_llm, ModelChoice { source: LlmSource::Anthropic, model: "claude-haiku-5-5".into() });
        // An empty task model takes the old default model.
        assert_eq!(s.translation_llm.model, "claude-sonnet-5-5");
        assert_eq!(s.notes_llm.model, "claude-opus-5-5");
        assert_eq!(s.planner_llm, s.notes_llm);
        assert_eq!(s.agent_model, "claude-opus-5-5");
    }

    #[test]
    fn v3_open_ai_compatible_file_migrates_to_server() {
        let text = r#"{"settingsVersion":3,"llmProvider":"openAiCompatible","llmBaseUrl":"http://localhost:1234/v1","llmModel":"qwen2.5","dictationModel":"llama3.2","notesModel":""}"#;
        let mut s = parse(text);
        migrate(&mut s, |_| false);
        assert_eq!(s.dictation_llm, ModelChoice { source: LlmSource::Server, model: "llama3.2".into() });
        assert_eq!(s.translation_llm, ModelChoice { source: LlmSource::Server, model: "qwen2.5".into() });
        assert_eq!(s.notes_llm, ModelChoice { source: LlmSource::Server, model: "qwen2.5".into() });
        assert_eq!(s.planner_llm, s.notes_llm);
        assert_eq!(s.llm_base_url, "http://localhost:1234/v1");
    }

    #[test]
    fn v4_file_is_not_migrated_again() {
        let text = r#"{"settingsVersion":4,"notesLlm":{"source":"openAi","model":"gpt-x"},"notesModel":"old"}"#;
        let s = parse(text);
        assert_eq!(s.notes_llm, ModelChoice { source: LlmSource::OpenAi, model: "gpt-x".into() });
        assert_eq!(s.dictation_llm.model, "claude-haiku-5-5");
    }

    #[test]
    fn v1_cloud_engine_with_key_is_kept() {
        let mut s = Settings { settings_version: 1, stt_engine: SttEngineKind::Deepgram, ..Default::default() };
        migrate(&mut s, |p| p == "deepgram");
        assert_eq!(s.stt_engine, SttEngineKind::Deepgram);
    }

    #[test]
    fn v2_choice_is_never_changed() {
        let mut s = Settings { stt_engine: SttEngineKind::OpenAiCompatible, ..Default::default() };
        migrate(&mut s, |_| false);
        assert_eq!(s.stt_engine, SttEngineKind::OpenAiCompatible);
    }

    #[test]
    fn choice_for_routes_each_task() {
        let s = Settings {
            planner_llm: ModelChoice { source: LlmSource::Server, model: " llama3.2 ".into() },
            ..Default::default()
        };
        assert_eq!(s.choice_for(LlmTask::Planner), ModelChoice { source: LlmSource::Server, model: "llama3.2".into() });
        assert_eq!(s.choice_for(LlmTask::Dictation).model, "claude-haiku-5-5");
        assert_eq!(s.choice_for(LlmTask::Agent), ModelChoice { source: LlmSource::Anthropic, model: "claude-opus-5-5".into() });
        let s = Settings { agent_model: " ".into(), ..Default::default() };
        assert_eq!(s.choice_for(LlmTask::Agent).model, crate::llm::DEFAULT_MODEL);
    }
}
