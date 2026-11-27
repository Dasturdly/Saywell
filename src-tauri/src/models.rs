//! Model catalogue and first-run download. This is the ONLY code in Saywell that uses the network.
//!
//! Downloads go to `<file>.part`, resume after an interruption (HTTP Range), and are renamed into
//! place only after the size and SHA-256 hash match the pinned values below.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ModelSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub const WHISPER: ModelSpec = ModelSpec {
    id: "speech",
    label: "Speech recognition (Whisper large-v3-turbo)",
    file: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    size: 574_041_195,
    sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
};

pub const LLM: ModelSpec = ModelSpec {
    id: "cleanup",
    label: "Text cleanup AI (Qwen3 4B Instruct)",
    file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    size: 2_497_281_120,
    sha256: "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
};

pub fn catalogue() -> [ModelSpec; 2] {
    [WHISPER, LLM]
}

pub fn by_id(id: &str) -> Option<ModelSpec> {
    catalogue().into_iter().find(|m| m.id == id)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download `spec` from `url` into `dir`. `progress(done, total)` is called as bytes arrive.
/// Set `cancel` to stop; the partial file is kept so the next attempt resumes.
pub fn download(
    spec: &ModelSpec,
    url: &str,
    dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(spec.file);
    let part = dir.join(format!("{}.part", spec.file));

    // Re-hash what we already have so a resumed download is still fully verified.
    let mut hasher = Sha256::new();
    let mut have = 0u64;
    if let Ok(mut f) = File::open(&part) {
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            have += n as u64;
        }
    }
    if have > spec.size {
        std::fs::remove_file(&part)?;
        hasher = Sha256::new();
        have = 0;
    }

    if have < spec.size {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(None)
            .user_agent("Saywell-model-downloader")
            .build()?;
        let mut req = client.get(url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let mut resp = req.send().with_context(|| format!("connecting to {url}"))?;
        let status = resp.status();
        if have > 0 && status == reqwest::StatusCode::OK {
            // Server ignored the Range header: start over.
            hasher = Sha256::new();
            have = 0;
        } else if !status.is_success() {
            bail!("download failed: HTTP {status}");
        }
        let mut out = OpenOptions::new().create(true).write(true).append(have > 0).truncate(have == 0).open(&part)?;
        let mut buf = vec![0u8; 1 << 16];
        progress(have, spec.size);
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let n = resp.read(&mut buf).context("download interrupted")?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            have += n as u64;
            if have > spec.size {
                break;
            }
            progress(have, spec.size);
        }
        out.flush()?;
    }

    if have != spec.size {
        if have > spec.size {
            let _ = std::fs::remove_file(&part);
        }
        bail!("download incomplete: got {have} of {} bytes", spec.size);
    }
    let got = hex(&hasher.finalize());
    if got != spec.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("{} failed verification (SHA-256 {got}); the file was deleted, please retry", spec.file);
    }
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

/// Which catalogue models are present in `dir` with the right size (a quick check; downloads
/// are hash-verified, and a damaged file shows up as a load error later).
pub fn present_in(dir: &Path) -> Vec<&'static str> {
    catalogue()
        .iter()
        .filter(|m| std::fs::metadata(dir.join(m.file)).is_ok_and(|md| md.len() == m.size))
        .map(|m| m.id)
        .collect()
}

/// Hash an existing file and compare with the catalogue (used for "I already have the file").
pub fn verify_file(spec: &ModelSpec, path: &Path) -> Result<()> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if len != spec.size {
        return Err(anyhow!("{} has {len} bytes, expected {}", path.display(), spec.size));
    }
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got = hex(&hasher.finalize());
    if got != spec.sha256 {
        bail!("{} does not match the expected SHA-256", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// Minimal HTTP server for `body`. Honours `Range: bytes=N-` when `ranges` is true.
    /// Serves `requests` requests, then exits. Returns the base URL.
    fn serve(body: Vec<u8>, ranges: bool, requests: usize) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in l.incoming().take(requests) {
                let mut s = stream.unwrap();
                let mut start = 0usize;
                let mut r = BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("range: bytes=") {
                        start = v.trim().trim_end_matches('-').parse().unwrap();
                    }
                }
                let (status, part) = if ranges && start > 0 { ("206 Partial Content", &body[start..]) } else { ("200 OK", &body[..]) };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", part.len());
                s.write_all(head.as_bytes()).unwrap();
                s.write_all(part).unwrap();
            }
        });
        url
    }

    fn spec_for(body: &[u8]) -> ModelSpec {
        let sha = Box::leak(hex(&Sha256::digest(body)).into_boxed_str());
        ModelSpec { id: "t", label: "test", file: "model.bin", url: "", size: body.len() as u64, sha256: sha }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("saywell-dl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn downloads_and_verifies() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let spec = spec_for(&body);
        let dir = tmp("ok");
        let mut last = (0, 0);
        let path = download(&spec, &serve(body.clone(), false, 1), &dir, &AtomicBool::new(false), |d, t| last = (d, t)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(last, (body.len() as u64, body.len() as u64));
        verify_file(&spec, &path).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resumes_a_partial_download() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 13) as u8).collect();
        let spec = spec_for(&body);
        let dir = tmp("resume");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.bin.part"), &body[..70_000]).unwrap();
        let path = download(&spec, &serve(body.clone(), true, 1), &dir, &AtomicBool::new(false), |_, _| {}).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), body);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_a_corrupt_file() {
        let body = vec![7u8; 50_000];
        let mut spec = spec_for(&body);
        spec.sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let dir = tmp("bad");
        let err = download(&spec, &serve(body, false, 1), &dir, &AtomicBool::new(false), |_, _| {}).unwrap_err();
        assert!(format!("{err}").contains("verification"), "{err}");
        assert!(!dir.join("model.bin").exists() && !dir.join("model.bin.part").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
