//! The MobileCLIP-S0 vision model: found in the cache, or downloaded once
//! from a pinned Hugging Face revision and checked against a SHA-256, then
//! loaded on a background thread. Until it is ready, covers get neutral
//! recipes that are not cached.
//!
//! The model's weights are Apple's, licensed for research/personal use
//! (`apple-amlr`). They are fetched on the user's machine and never shipped.

use cw_analysis::ClipModel;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MODEL_URL: &str = "https://huggingface.co/Xenova/mobileclip_s0/resolve/757d59c9c6870a76a4b0306f05f5061bca15c39f/onnx/vision_model.onnx";
const MODEL_SHA256: &str = "17d3c037b1d488c10c50e09f6009ea5a198caef4e0e8f4ea5617b7cb2d067ac0";
const MODEL_FILE: &str = "mobileclip_s0_vision.onnx";

#[derive(Clone)]
pub enum ModelStatus {
    /// Checking the cache or about to download.
    Starting,
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Loading,
    Ready(Arc<ClipModel>),
    /// Not available; the reason is shown in the debug overlay.
    Unavailable(String),
}

impl ModelStatus {
    pub fn describe(&self) -> String {
        match self {
            ModelStatus::Starting => "starting".into(),
            ModelStatus::Downloading { done, total } => match total {
                Some(t) if *t > 0 => format!(
                    "downloading {:.0}/{:.0} MB",
                    *done as f64 / 1e6,
                    *t as f64 / 1e6
                ),
                _ => format!("downloading {:.0} MB", *done as f64 / 1e6),
            },
            ModelStatus::Loading => "loading".into(),
            ModelStatus::Ready(_) => "ready".into(),
            ModelStatus::Unavailable(why) => why.clone(),
        }
    }
}

/// Shared view of the model; cheap to clone.
#[derive(Clone)]
pub struct ModelHandle(Arc<Mutex<ModelStatus>>);

impl ModelHandle {
    pub fn status(&self) -> ModelStatus {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn ready(&self) -> Option<Arc<ClipModel>> {
        match self.status() {
            ModelStatus::Ready(m) => Some(m),
            _ => None,
        }
    }

    fn set(&self, s: ModelStatus) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// A handle that never becomes ready.
    #[cfg(test)]
    pub fn unavailable(why: &str) -> ModelHandle {
        ModelHandle(Arc::new(Mutex::new(ModelStatus::Unavailable(why.into()))))
    }
}

/// `$XDG_CACHE_HOME/coverwave`, else `~/.cache/coverwave`.
pub fn cache_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// `$XDG_CONFIG_HOME/coverwave`, else `~/.config/coverwave`.
pub fn config_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    let base = std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))?;
    Some(base.join("coverwave"))
}

/// Starts finding (or, unless `offline`, downloading) and loading the model
/// on a background thread. `COVERWAVE_MODEL` points at a local ONNX file
/// instead (no checksum).
pub fn start(offline: bool) -> ModelHandle {
    let handle = ModelHandle(Arc::new(Mutex::new(ModelStatus::Starting)));
    let h = handle.clone();
    let spawned = std::thread::Builder::new()
        .name("cw-model".into())
        .spawn(move || {
            let result = obtain(&h, offline).and_then(|bytes| {
                h.set(ModelStatus::Loading);
                ClipModel::from_onnx_bytes(&bytes).map_err(|e| e.to_string())
            });
            h.set(match result {
                Ok(m) => ModelStatus::Ready(Arc::new(m)),
                Err(e) => ModelStatus::Unavailable(e),
            });
        });
    if let Err(e) = spawned {
        handle.set(ModelStatus::Unavailable(format!(
            "cannot start loader: {e}"
        )));
    }
    handle
}

fn obtain(h: &ModelHandle, offline: bool) -> Result<Vec<u8>, String> {
    if let Some(p) = std::env::var_os("COVERWAVE_MODEL") {
        return std::fs::read(&p).map_err(|e| format!("COVERWAVE_MODEL: {e}"));
    }
    let dir = cache_dir()
        .ok_or("no cache directory (HOME unset)")?
        .join("models");
    let path = dir.join(MODEL_FILE);
    if let Ok(bytes) = std::fs::read(&path) {
        if sha256_hex(&bytes) == MODEL_SHA256 {
            return Ok(bytes);
        }
        // Corrupt or from another revision: fetch it again.
        let _ = std::fs::remove_file(&path);
    }
    if offline {
        return Err("no model (offline)".into());
    }
    download(h, &dir, &path)
}

fn download(h: &ModelHandle, dir: &Path, path: &Path) -> Result<Vec<u8>, String> {
    use ureq::tls::{RootCerts, TlsConfig};
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let resp = agent
        .get(MODEL_URL)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let mut reader = resp.into_body().into_reader();
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        h.set(ModelStatus::Downloading {
            done: bytes.len() as u64,
            total,
        });
    }
    drop(file);
    if sha256_hex(&bytes) != MODEL_SHA256 {
        let _ = std::fs::remove_file(&part);
        return Err("downloaded model failed its checksum".into());
    }
    std::fs::rename(&part, path).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_of_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
