//! Microphone capture (cpal / WASAPI) on a dedicated thread, plus WAV helpers.
//!
//! cpal streams are not `Send`, so one thread owns the stream and is driven by commands.
//! The mic is opened only while recording, so Windows' "mic in use" indicator stays off when idle.

use std::io::Cursor;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub const TARGET_RATE: u32 = 16_000;

/// Audio ready for speech-to-text: 16 kHz mono f32.
#[derive(Debug, Clone, Default)]
pub struct Clip {
    pub samples: Vec<f32>,
}

impl Clip {
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / TARGET_RATE as f32
    }
}

enum Cmd {
    Start,
    Stop(Sender<Result<Clip>>),
}

#[derive(Clone)]
pub struct Recorder {
    tx: Sender<Cmd>,
}

impl Recorder {
    /// Spawn the capture thread.
    pub fn spawn(max_secs: u32) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || audio_thread(rx, max_secs))
            .expect("spawn audio thread");
        Self { tx }
    }

    pub fn start(&self) {
        let _ = self.tx.send(Cmd::Start);
    }

    /// Stop recording and return the captured clip (resampled to 16 kHz mono).
    pub fn stop(&self) -> Result<Clip> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Cmd::Stop(tx)).map_err(|_| anyhow!("audio thread gone"))?;
        rx.recv().map_err(|_| anyhow!("audio thread gone"))?
    }
}

struct Active {
    _stream: cpal::Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

fn audio_thread(rx: Receiver<Cmd>, max_secs: u32) {
    let mut active: Option<Active> = None;
    for cmd in rx {
        match cmd {
            Cmd::Start => {
                active = None; // drop any previous stream first
                let t0 = Instant::now();
                match open_stream(max_secs) {
                    Ok(a) => {
                        log::info!("mic open in {} ms at {} Hz", t0.elapsed().as_millis(), a.rate);
                        active = Some(a);
                    }
                    Err(e) => log::error!("could not open microphone: {e:#}"),
                }
            }
            Cmd::Stop(reply) => {
                let res = match active.take() {
                    Some(a) => {
                        let rate = a.rate;
                        let raw = std::mem::take(&mut *a.buf.lock().unwrap());
                        drop(a); // closes the mic
                        Ok(Clip { samples: resample_to_16k(&raw, rate) })
                    }
                    None => Err(anyhow!("not recording (microphone failed to open)")),
                };
                let _ = reply.send(res);
            }
        }
    }
}

fn open_stream(max_secs: u32) -> Result<Active> {
    let host = cpal::default_host();
    let device = host.default_input_device().context("no default microphone")?;
    let config = device.default_input_config().context("reading mic config")?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let cap = rate as usize * max_secs as usize;
    let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(rate as usize * 30)));
    let stream_cfg: cpal::StreamConfig = config.clone().into();
    let err = |e| log::error!("mic stream error: {e}");

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &stream_cfg, channels, cap, buf.clone(), err)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, &stream_cfg, channels, cap, buf.clone(), err)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, &stream_cfg, channels, cap, buf.clone(), err)?,
        cpal::SampleFormat::I32 => build::<i32>(&device, &stream_cfg, channels, cap, buf.clone(), err)?,
        other => return Err(anyhow!("unsupported mic sample format {other:?}")),
    };
    stream.play()?;
    Ok(Active { _stream: stream, buf, rate })
}

fn build<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    channels: usize,
    cap: usize,
    buf: Arc<Mutex<Vec<f32>>>,
    err: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let stream = device.build_input_stream(
        cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut b = buf.lock().unwrap();
            for frame in data.chunks(channels) {
                if b.len() >= cap {
                    return;
                }
                let sum: f32 = frame.iter().map(|s| s.to_sample::<f32>()).sum();
                b.push(sum / channels as f32);
            }
        },
        err,
        None,
    )?;
    Ok(stream)
}

/// Downsample mono audio to 16 kHz: box low-pass over the decimation window, then linear interpolation.
/// Good enough for speech recognition; avoids a resampler dependency.
pub fn resample_to_16k(input: &[f32], rate: u32) -> Vec<f32> {
    if rate == TARGET_RATE || input.is_empty() {
        return input.to_vec();
    }
    let ratio = rate as f64 / TARGET_RATE as f64;
    let filtered: Vec<f32> = if ratio > 1.0 {
        let w = ratio.round().max(1.0) as usize;
        let mut out = Vec::with_capacity(input.len());
        let mut acc = 0.0f32;
        for i in 0..input.len() {
            acc += input[i];
            if i >= w {
                acc -= input[i - w];
            }
            out.push(acc / (i + 1).min(w) as f32);
        }
        out
    } else {
        input.to_vec()
    };
    let n_out = (input.len() as f64 / ratio).floor() as usize;
    (0..n_out)
        .map(|i| {
            let pos = i as f64 * ratio;
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(filtered.len() - 1);
            let frac = (pos - i0 as f64) as f32;
            filtered[i0] * (1.0 - frac) + filtered[i1] * frac
        })
        .collect()
}

/// Read any PCM WAV file into a 16 kHz mono clip.
pub fn read_wav(path: &Path) -> Result<Clip> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = interleaved.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    Ok(Clip { samples: resample_to_16k(&mono, spec.sample_rate) })
}

/// Encode a clip as an in-memory 16-bit WAV file.
pub fn encode_wav(clip: &Clip) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cur = Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec)?;
        for &s in &clip.samples {
            w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        w.finalize()?;
    }
    Ok(cur.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_lengths() {
        let one_sec_48k = vec![0.1f32; 48_000];
        assert_eq!(resample_to_16k(&one_sec_48k, 48_000).len(), 16_000);
        let one_sec_44k = vec![0.1f32; 44_100];
        assert_eq!(resample_to_16k(&one_sec_44k, 44_100).len(), 16_000);
        assert_eq!(resample_to_16k(&[0.5; 160], 16_000).len(), 160);
    }

    #[test]
    fn resample_keeps_dc_level() {
        let out = resample_to_16k(&vec![0.25f32; 48_000], 48_000);
        assert!(out[100..].iter().all(|v| (v - 0.25).abs() < 1e-4));
    }

    #[test]
    fn wav_round_trip() {
        let clip = Clip { samples: (0..1600).map(|i| (i as f32 / 50.0).sin() * 0.5).collect() };
        let bytes = encode_wav(&clip).unwrap();
        let path = std::env::temp_dir().join(format!("saywell-rt-{}.wav", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let back = read_wav(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.samples.len(), clip.samples.len());
        assert!(back.samples.iter().zip(&clip.samples).all(|(a, b)| (a - b).abs() < 1e-3));
    }
}
