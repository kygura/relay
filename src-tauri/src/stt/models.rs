//! Local Whisper model catalog and downloader. Ported from VoxFlow's
//! `core/src/models.rs`: pinned Hugging Face revision, size and sha256 checked
//! before the file is renamed into place.

use anyhow::{bail, ensure, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WhisperModelInfo {
    pub name: &'static str,
    pub label: &'static str,
    /// Download size in decimal MB.
    pub size_mb: u32,
    pub english_only: bool,
    pub downloaded: bool,
    #[serde(skip)]
    size: u64,
    #[serde(skip)]
    sha256: &'static str,
}

const fn m(name: &'static str, label: &'static str, size: u64, english_only: bool, sha256: &'static str) -> WhisperModelInfo {
    WhisperModelInfo {
        name,
        label,
        size_mb: ((size + 500_000) / 1_000_000) as u32,
        english_only,
        downloaded: false,
        size,
        sha256,
    }
}

/// Pinned commit of huggingface.co/ggerganov/whisper.cpp.
const REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

pub const DEFAULT_MODEL: &str = "small-q5_1";

#[rustfmt::skip]
pub const CATALOG: &[WhisperModelInfo] = &[
    m("tiny", "Tiny (fastest, rough)", 77_691_713, false, "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21"),
    m("base", "Base (fast)", 147_951_465, false, "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"),
    m("base.en", "Base, English only", 147_964_211, true, "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"),
    m("small-q5_1", "Small, quantized (default)", 190_085_487, false, "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"),
    m("small", "Small", 487_601_967, false, "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"),
    m("small.en", "Small, English only", 487_614_201, true, "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d"),
    m("large-v3-turbo-q5_0", "Large v3 turbo, quantized (best quality)", 574_041_195, false, "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"),
    m("medium", "Medium", 1_533_763_059, false, "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208"),
];

pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

pub fn list(dir: &Path) -> Vec<WhisperModelInfo> {
    CATALOG
        .iter()
        .map(|info| WhisperModelInfo {
            downloaded: dir.join(file_name(info.name)).is_file(),
            ..info.clone()
        })
        .collect()
}

fn file_name(name: &str) -> String {
    format!("ggml-{name}.bin")
}

/// Path of a catalog model. Unknown names are rejected, which also blocks path traversal.
pub fn model_path(dir: &Path, name: &str) -> Result<PathBuf> {
    info(name)?;
    Ok(dir.join(file_name(name)))
}

fn info(name: &str) -> Result<&'static WhisperModelInfo> {
    CATALOG
        .iter()
        .find(|m| m.name == name)
        .with_context(|| format!("Unknown Whisper model: {name}"))
}

/// Download a catalog model into `dir` via `<file>.part` and rename.
/// `progress(done, total)` is called about every 100 ms.
pub async fn download(
    dir: &Path,
    name: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    let path = model_path(dir, name)?;
    let info = info(name)?;
    tokio::fs::create_dir_all(dir).await.with_context(|| format!("create {}", dir.display()))?;
    let part = path.with_extension("bin.part");
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REVISION}/{}",
        file_name(name)
    );
    let result: Result<()> = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .build()?;
        let resp = client.get(&url).send().await?.error_for_status()?;
        let total = resp.content_length().unwrap_or(info.size);
        let mut file = tokio::fs::File::create(&part)
            .await
            .with_context(|| format!("create {}", part.display()))?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        let mut last = Instant::now();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                bail!("Download cancelled");
            }
            let chunk = chunk.context("Download interrupted")?;
            file.write_all(&chunk).await?;
            hasher.update(&chunk);
            done += chunk.len() as u64;
            if last.elapsed() >= Duration::from_millis(100) {
                progress(done, total);
                last = Instant::now();
            }
        }
        progress(done, total);
        ensure!(done == info.size, "Download incomplete: got {done} of {} bytes", info.size);
        ensure!(hex(&hasher.finalize()) == info.sha256, "Checksum mismatch for {name}");
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&part, &path).await?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    Ok(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_path_rejects_unknown_and_traversal() {
        let dir = Path::new("/models");
        assert!(model_path(dir, "../x").is_err());
        assert!(model_path(dir, "").is_err());
        assert_eq!(model_path(dir, "base.en").unwrap(), dir.join("ggml-base.en.bin"));
    }

    #[test]
    fn default_is_in_catalog_and_hashes_are_well_formed() {
        assert!(info(DEFAULT_MODEL).is_ok());
        for m in CATALOG {
            assert_eq!(m.sha256.len(), 64, "{}", m.name);
            assert!(m.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        }
    }
}
