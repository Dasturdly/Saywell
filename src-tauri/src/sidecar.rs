//! Launching helper servers (whisper-server, later llama-server) as hidden child processes.
//!
//! Each child is placed in a Windows job object with KILL_ON_JOB_CLOSE, so it dies with Saywell
//! even if Saywell crashes. Servers listen on 127.0.0.1 on a free port.

use std::fs::File;
use std::net::TcpListener;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Sidecar {
    pub name: &'static str,
    pub port: u16,
    child: Mutex<Option<Child>>,
}

impl Sidecar {
    /// Start `exe` with `args` plus `--host 127.0.0.1 --port <free>`; stdout/stderr go to `log_path`.
    pub fn start(name: &'static str, exe: &Path, args: &[String], log_path: &Path) -> Result<Self> {
        if !exe.exists() {
            bail!("{name}: executable not found at {}", exe.display());
        }
        let port = free_port()?;
        if let Some(dir) = log_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let log = File::create(log_path)?;
        let child = Command::new(exe)
            .args(args)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .current_dir(exe.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .with_context(|| format!("starting {name}"))?;
        if let Err(e) = kill_with_parent(&child) {
            log::warn!("{name}: could not attach to job object: {e:#}");
        }
        log::info!("{name}: started pid {} on port {port}", child.id());
        Ok(Self { name, port, child: Mutex::new(Some(child)) })
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Poll `path` until it answers 2xx, the process exits, or `timeout` passes.
    pub fn wait_ready(&self, path: &str, timeout: Duration) -> Result<Duration> {
        let t0 = Instant::now();
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(2)).build()?;
        let url = format!("{}{}", self.base_url(), path);
        while t0.elapsed() < timeout {
            if let Some(status) = self.child.lock().unwrap().as_mut().and_then(|c| c.try_wait().ok().flatten()) {
                bail!("{} exited during startup ({status})", self.name);
            }
            if client.get(&url).send().map(|r| r.status().is_success()).unwrap_or(false) {
                return Ok(t0.elapsed());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("{} did not become ready within {:?}", self.name, timeout)
    }

    pub fn stop(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
            log::info!("{}: stopped", self.name);
        }
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn kill_with_parent(child: &Child) -> Result<()> {
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
        AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()))?;
        // The job handle is intentionally leaked: it closes (killing the child) when Saywell exits.
    }
    Ok(())
}

/// Find a bundled helper binary: next to the running exe first, then the source tree (dev builds).
pub fn find_binary(subdir: &str, exe: &str) -> PathBuf {
    let rel = Path::new("binaries").join(subdir).join(exe);
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let p = dir.join(&rel);
        if p.exists() {
            return p;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}
