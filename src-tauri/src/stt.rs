//! Speech-to-text through a warm whisper-server sidecar (whisper.cpp built with Vulkan).

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::multipart::{Form, Part};

use crate::audio::{encode_wav, Clip};
use crate::sidecar::{find_binary, Sidecar};

pub struct Stt {
    server: Sidecar,
    client: reqwest::blocking::Client,
}

impl Stt {
    /// Start whisper-server with `model` and wait until it has loaded the model.
    pub fn start(model: &Path, log_dir: &Path) -> Result<Self> {
        if !model.exists() {
            bail!("whisper model not found at {}", model.display());
        }
        let exe = find_binary("whisper", "whisper-server.exe");
        let args = vec![
            "-m".to_string(),
            model.display().to_string(),
            "-l".into(),
            "en".into(),
            "-nt".into(),
        ];
        let server = Sidecar::start("whisper-server", &exe, &args, &log_dir.join("whisper-server.log"))?;
        let took = server.wait_ready("/", Duration::from_secs(90))?;
        log::info!("whisper-server ready in {} ms", took.as_millis());
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(120)).build()?;
        let stt = Self { server, client };
        // Warm-up so the first real dictation doesn't pay shader/graph setup cost.
        let _ = stt.transcribe(&Clip { samples: vec![0.0; 16_000] });
        Ok(stt)
    }

    /// Transcribe a 16 kHz mono clip. Returns Whisper's raw text.
    pub fn transcribe(&self, clip: &Clip) -> Result<String> {
        let wav = encode_wav(clip)?;
        let form = Form::new()
            .part("file", Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav")?)
            .text("response_format", "text")
            .text("temperature", "0");
        let resp = self
            .client
            .post(format!("{}/inference", self.server.base_url()))
            .multipart(form)
            .send()
            .context("whisper-server request failed")?;
        let status = resp.status();
        let body = resp.text()?;
        if !status.is_success() {
            bail!("whisper-server returned {status}: {body}");
        }
        Ok(body)
    }

    pub fn stop(&self) {
        self.server.stop();
    }
}
