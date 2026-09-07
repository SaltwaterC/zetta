//! Opt-in metadata-only tracing. Producers never wait for disk I/O.
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static TRACE: OnceLock<SyncSender<Event>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);
const LIMIT: u64 = 64 * 1024 * 1024;

enum Event {
    Record {
        at: Instant,
        name: &'static str,
        a: u64,
        b: u64,
    },
    Stop(SyncSender<()>),
}

/// Open before daemonization so configuration errors reach the SSH caller.
pub fn open() -> Result<Option<File>> {
    let Some(path) = std::env::var_os("MOSH_SERVER_TIMING_LOG") else {
        return Ok(None);
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)
        .with_context(|| {
            format!("creating timing log {path:?}; use a new file in an existing directory")
        })
        .map(Some)
}

/// Start only after the final fork; a logger thread must never cross fork.
pub fn start(file: Option<File>) {
    let Some(file) = file else { return };
    let origin = Instant::now();
    let (tx, rx) = mpsc::sync_channel::<Event>(4096);
    if TRACE.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let _ = write_events(file, rx, origin);
    });
}

fn write_events(mut file: impl Write, rx: Receiver<Event>, origin: Instant) -> std::io::Result<()> {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    writeln!(
        file,
        "# zosh timing v1 pid={} unix_ms={epoch} os={} arch={}\n# elapsed_us event a b dropped_total",
        std::process::id(),
        std::env::consts::OS,
        std::env::consts::ARCH
    )?;
    let mut written = 0;
    while let Ok(event) = rx.recv() {
        match event {
            Event::Record { at, name, a, b } => {
                let line = format!(
                    "{} {name} {a} {b} {}\n",
                    at.saturating_duration_since(origin).as_micros(),
                    DROPPED.load(Ordering::Relaxed)
                );
                file.write_all(line.as_bytes())?;
                written += line.len() as u64;
                if written >= LIMIT {
                    writeln!(file, "# size limit reached; tracing stopped")?;
                    break;
                }
            }
            Event::Stop(done) => {
                file.flush()?;
                let _ = done.send(());
                break;
            }
        }
    }
    Ok(())
}

pub fn record(name: &'static str, a: u64, b: u64) {
    if let Some(tx) = TRACE.get()
        && tx
            .try_send(Event::Record {
                at: Instant::now(),
                name,
                a,
                b,
            })
            .is_err()
    {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn finish() {
    if let Some(tx) = TRACE.get() {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        if tx.try_send(Event::Stop(done_tx)).is_ok() {
            // A slow or failed log disk must not hold a closing session open.
            let _ = done_rx.recv_timeout(Duration::from_millis(250));
        }
    }
}

pub fn begin() -> Option<Instant> {
    TRACE.get().map(|_| Instant::now())
}

pub fn slow(name: &'static str, since: Option<Instant>) {
    if let Some(since) = since {
        let elapsed = since.elapsed();
        if elapsed >= Duration::from_millis(100) {
            record(name, elapsed.as_micros() as u64, 0);
        }
    }
}

pub struct LoopTiming {
    last: Option<Instant>,
    heartbeat: Option<Instant>,
}

impl LoopTiming {
    pub fn new() -> Self {
        Self {
            last: begin(),
            heartbeat: begin(),
        }
    }

    pub fn tick(&mut self) {
        slow("loop_gap", self.last);
        self.last = begin();
        if self
            .heartbeat
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
        {
            record("heartbeat", 0, 0);
            self.heartbeat = self.last;
        }
    }
}

#[cfg(test)]
#[path = "tests/timing.rs"]
mod tests;
