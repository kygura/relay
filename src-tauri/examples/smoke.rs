//! End-to-end smoke test against the real APIs, using the keys stored by the app.
//!
//!   cargo run --example smoke -- [path\to\speech.wav]
//!
//! Steps: Claude polish, translate and meeting notes, then speech to text on the
//! WAV file with the engine selected in Settings. Prints timings and results.

use relay_lib::llm::LlmClient;
use relay_lib::settings::LlmTask;
use relay_lib::types::*;
use std::time::Instant;
use tokio::sync::mpsc;

fn seg(id: &str, source: AudioSource, speaker: &str, text: &str, start_ms: u64) -> TranscriptSegment {
    TranscriptSegment {
        id: id.into(),
        source,
        speaker: Some(speaker.into()),
        text: text.into(),
        start_ms,
        end_ms: start_ms + 4_000,
        is_final: true,
        language: Some("en".into()),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("no config dir"))?
        .join("dev.relay.assistant");
    let settings = relay_lib::settings::load(&config_dir);
    println!(
        "dictation: {:?}  stt: {:?}  targets: {:?}",
        settings.choice_for(LlmTask::Dictation),
        settings.stt_engine,
        settings.target_languages
    );

    let mut failures = 0;

    // 1. Model: polish (Claude or the OpenAI-compatible server, as set in Settings)
    let client = LlmClient::for_task(&settings, LlmTask::Dictation)?;
    let raw = "um so I wanted to uh tell you that the meeting is on Monday no wait make that Tuesday at three and uh bring the the budget numbers";
    println!("\n[basic cleanup] {}", relay_lib::pipeline::cleanup::basic(raw, "en"));
    let t = Instant::now();
    match relay_lib::pipeline::polish(
        &client,
        raw,
        &PolishContext {
            instructions: "Casual chat message.".into(),
            match_app: false,
            window: None,
            user_profile: String::new(),
        },
    )
    .await
    {
        Ok(out) => println!("\n[polish ok, {} ms]\n{out}", t.elapsed().as_millis()),
        Err(e) => {
            failures += 1;
            println!("\n[polish FAILED] {e:#}");
        }
    }

    // 2. Claude: translate
    let segments = vec![
        seg("a", AudioSource::System, "Speaker 1", "Thanks everyone for joining. We have three items on the agenda today.", 0),
        seg("b", AudioSource::Microphone, "Me", "First, the launch date moves to March 14 because QA found two blocking bugs.", 5_000),
        seg("c", AudioSource::System, "Speaker 2", "I can fix the login bug by Friday. Priya owns the payment bug.", 10_000),
        seg("d", AudioSource::System, "Speaker 1", "Open question: do we still need the press release before launch?", 15_000),
    ];
    let mut targets = settings.target_languages.clone();
    if targets.is_empty() {
        targets.push("es".into());
    }
    let t = Instant::now();
    match relay_lib::pipeline::translate(&client, &segments[1], &targets, &segments[..1]).await {
        Ok(list) => {
            println!("\n[translate ok, {} ms]", t.elapsed().as_millis());
            for tr in list {
                println!("  {}: {}", tr.target_lang, tr.text);
            }
        }
        Err(e) => {
            failures += 1;
            println!("\n[translate FAILED] {e:#}");
        }
    }

    // 3. Claude: meeting notes
    let t = Instant::now();
    match relay_lib::pipeline::meeting_notes(&client, &segments, None).await {
        Ok(n) => println!(
            "\n[notes ok, {} ms]\nsummary: {}\ndecisions: {:?}\nactions: {:?}\nquestions: {:?}",
            t.elapsed().as_millis(),
            n.summary,
            n.decisions,
            n.action_items.iter().map(|a| format!("{} ({:?}, {:?})", a.text, a.owner, a.due)).collect::<Vec<_>>(),
            n.open_questions
        ),
        Err(e) => {
            failures += 1;
            println!("\n[notes FAILED] {e:#}");
        }
    }

    // 4. Speech to text on a WAV file
    if let Some(path) = std::env::args().nth(1) {
        let mut reader = hound::WavReader::open(&path)?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => reader.samples::<i16>().map(|s| s.unwrap_or(0) as f32 / 32768.0).collect(),
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        };
        let mono = relay_lib::audio::resample::downmix(&samples, spec.channels as usize);
        let audio = relay_lib::audio::resample::resample_once(&mono, spec.sample_rate, SAMPLE_RATE);
        println!("\n[stt] {} s of audio from {path}", audio.len() / SAMPLE_RATE as usize);

        // Split with the same VAD the app uses.
        let mut vad = relay_lib::audio::vad::Vad::new(relay_lib::audio::vad::VadConfig {
            threshold: settings.vad_threshold,
            silence_ms: settings.silence_ms,
            max_chunk_ms: settings.max_chunk_ms,
        });
        let mut chunks = vad.push(&audio);
        chunks.extend(vad.flush());
        println!("[stt] VAD produced {} chunk(s)", chunks.len());

        let models_dir = relay_lib::stt::models::models_dir(
            &dirs::config_dir().unwrap_or_default().join("dev.relay.assistant"),
        );
        let engine = relay_lib::stt::build_engine(&settings, &models_dir, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)))?;
        let (ctx, crx) = mpsc::channel(32);
        let (stx, mut srx) = mpsc::channel(64);
        let t = Instant::now();
        let run = tokio::spawn(async move { engine.run(crx, stx).await });
        for c in chunks {
            ctx.send(AudioChunk { source: AudioSource::Microphone, samples: c.samples, start_ms: c.start_ms, end_ms: c.end_ms }).await?;
        }
        drop(ctx);
        let mut got = 0;
        while let Some(s) = srx.recv().await {
            if s.is_final {
                got += 1;
                println!("  [{} ms] {}", t.elapsed().as_millis(), s.text);
            }
        }
        match run.await? {
            Ok(()) if got > 0 => println!("[stt ok]"),
            Ok(()) => {
                failures += 1;
                println!("[stt FAILED] no text returned");
            }
            Err(e) => {
                failures += 1;
                println!("[stt FAILED] {e:#}");
            }
        }
    } else {
        println!("\n[stt skipped] pass a WAV path to test speech to text");
    }

    println!("\n{} failure(s)", failures);
    if failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}
