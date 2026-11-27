//! Local LLM through a llama-server sidecar (llama.cpp, Vulkan), OpenAI-compatible chat API.
//!
//! The server listens on 127.0.0.1 only and requires a random per-run API key, so other local
//! programs cannot use it. If it crashes, it is restarted in the background; requests made while
//! it is down fail fast and the caller falls back to the uncleaned text.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::sidecar::{find_binary, Sidecar};

pub struct Llm {
    model: PathBuf,
    log_dir: PathBuf,
    api_key: String,
    server: Mutex<Option<Arc<Sidecar>>>,
    restarting: Arc<AtomicBool>,
    client: reqwest::blocking::Client,
}

impl Llm {
    /// Start llama-server with `model` and wait until the model is loaded.
    pub fn start(model: &Path, log_dir: &Path) -> Result<Arc<Self>> {
        if !model.exists() {
            bail!("LLM model not found at {}", model.display());
        }
        let client = crate::sidecar::loopback_client(Duration::from_secs(60))?;
        let llm = Arc::new(Self {
            model: model.to_path_buf(),
            log_dir: log_dir.to_path_buf(),
            api_key: random_key(),
            server: Mutex::new(None),
            restarting: Arc::new(AtomicBool::new(false)),
            client,
        });
        let server = llm.spawn_server()?;
        *llm.server.lock().unwrap() = Some(server);
        // Warm-up: the first request pays for shader compilation and graph setup.
        let _ = llm.chat("Reply with OK.", "ok", 8, Duration::from_secs(60));
        Ok(llm)
    }

    fn spawn_server(&self) -> Result<Arc<Sidecar>> {
        let exe = find_binary("llama", "llama-server.exe");
        let args: Vec<String> = [
            "-m",
            &self.model.display().to_string(),
            "-ngl",
            "99",
            "-c",
            "4096",
            "-np",
            "1",
            "-fa",
            "off",
            "--reasoning",
            "off",
            "--no-webui",
            "--no-slots",
            "--api-key",
            &self.api_key,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let server = Sidecar::start("llama-server", &exe, &args, &self.log_dir.join("llama-server.log"))?;
        let took = server.wait_ready("/health", Duration::from_secs(120))?;
        log::info!("llama-server ready in {} ms", took.as_millis());
        Ok(Arc::new(server))
    }

    /// Current server, or an error if it is down (and a restart is kicked off).
    fn live_server(self: &Arc<Self>) -> Result<Arc<Sidecar>> {
        let current = self.server.lock().unwrap().clone();
        match current {
            Some(s) if !s.has_exited() => Ok(s),
            _ => {
                self.restart_in_background();
                Err(anyhow!("llama-server is not running (restarting)"))
            }
        }
    }

    fn restart_in_background(self: &Arc<Self>) {
        if self.restarting.swap(true, Ordering::SeqCst) {
            return;
        }
        log::warn!("llama-server is down; restarting");
        let me = self.clone();
        std::thread::spawn(move || {
            match me.spawn_server() {
                Ok(s) => *me.server.lock().unwrap() = Some(s),
                Err(e) => log::error!("llama-server restart failed: {e:#}"),
            }
            me.restarting.store(false, Ordering::SeqCst);
        });
    }

    /// One chat completion at temperature 0, abandoned after `timeout`. Returns the assistant text.
    pub fn chat(self: &Arc<Self>, system: &str, user: &str, max_tokens: u32, timeout: Duration) -> Result<String> {
        self.chat_with_examples(system, &[], user, max_tokens, timeout)
    }

    /// Like [`chat`](Self::chat), with few-shot `(user, assistant)` example turns before `user`.
    pub fn chat_with_examples(
        self: &Arc<Self>,
        system: &str,
        examples: &[(&str, &str)],
        user: &str,
        max_tokens: u32,
        timeout: Duration,
    ) -> Result<String> {
        let server = self.live_server()?;
        let mut messages = vec![json!({"role": "system", "content": system})];
        for (u, a) in examples {
            messages.push(json!({"role": "user", "content": u}));
            messages.push(json!({"role": "assistant", "content": a}));
        }
        messages.push(json!({"role": "user", "content": user}));
        let body = json!({
            "messages": messages,
            "temperature": 0,
            "max_tokens": max_tokens,
        });
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", server.base_url()))
            .bearer_auth(&self.api_key)
            .timeout(timeout)
            .json(&body)
            .send()
            .context("llama-server request failed")?;
        let status = resp.status();
        let v: Value = resp.json().context("llama-server sent invalid JSON")?;
        if !status.is_success() {
            bail!("llama-server returned {status}: {v}");
        }
        v["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.trim().to_string())
            .ok_or_else(|| anyhow!("llama-server reply had no content"))
    }

    pub fn stop(&self) {
        if let Some(s) = self.server.lock().unwrap().take() {
            s.stop();
        }
    }
}

/// 128-bit random hex key from the OS-seeded hasher (no extra dependency).
fn random_key() -> String {
    let part = || {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}
