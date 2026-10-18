//! Debounced, bounded background configuration persistence. Audio callbacks and
//! window handlers never write to disk. Abrupt process termination retains the
//! last completed autosave rather than depending on the normal Quit path.
use crate::config::Config;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(100);
const DEBOUNCE: Duration = Duration::from_millis(300);
const MAX_WAIT: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_secs(2);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

struct SaveState {
    seen: Config,
    saved: Option<Config>,
    changed_at: Instant,
    dirty_since: Instant,
    retry_at: Option<Instant>,
}
impl SaveState {
    fn new(config: Config, now: Instant) -> Self {
        Self {
            seen: config,
            saved: None,
            changed_at: now,
            dirty_since: now,
            retry_at: None,
        }
    }
    fn observe(&mut self, config: Config, now: Instant) {
        if config != self.seen {
            if self.saved.as_ref() == Some(&self.seen) {
                self.dirty_since = now;
            }
            self.seen = config;
            self.changed_at = now;
        }
    }
    fn due(&self, now: Instant, final_save: bool) -> bool {
        self.saved.as_ref() != Some(&self.seen)
            && (final_save
                || (self.retry_at.is_none_or(|at| now >= at)
                    && (now.duration_since(self.changed_at) >= DEBOUNCE
                        || now.duration_since(self.dirty_since) >= MAX_WAIT)))
    }
    fn save(&mut self, path: &std::path::Path, now: Instant) {
        match self.seen.save_to_path(path) {
            Ok(()) => {
                self.saved = Some(self.seen.clone());
                self.retry_at = None;
                *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = None;
                tracing::debug!("Settings autosaved to {}", path.display());
            }
            Err(error) => {
                self.retry_at = Some(now + RETRY);
                let message = format!("Não foi possível salvar configurações: {error}");
                tracing::error!("{message}");
                *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(message);
            }
        }
    }
}

pub struct Autosave {
    stop: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
}
impl Autosave {
    pub fn start(path: PathBuf, snapshot: impl Fn() -> Config + Send + 'static) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (finished, done) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("settings-save".into())
            .spawn(move || {
                let mut state = SaveState::new(snapshot(), Instant::now());
                loop {
                    let final_save = worker_stop.load(Ordering::Acquire);
                    let now = Instant::now();
                    state.observe(snapshot(), now);
                    if state.due(now, final_save) {
                        state.save(&path, now);
                    }
                    if final_save {
                        break;
                    }
                    thread::park_timeout(POLL);
                }
                let _ = finished.try_send(());
            })?;
        Ok(Self {
            stop,
            done,
            worker: Some(worker),
        })
    }
    /// Flush while the pipeline still holds the user's desired state. Waiting
    /// is bounded so a stalled filesystem cannot hold shutdown indefinitely.
    pub fn finish(mut self) {
        self.stop_and_wait();
    }
    fn stop_and_wait(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stop.store(true, Ordering::Release);
            worker.thread().unpark();
            if self.done.recv_timeout(Duration::from_secs(2)).is_ok() {
                let _ = worker.join();
            } else {
                tracing::error!(
                    "Settings writer did not finish within 2 seconds; last completed autosave remains available"
                );
            }
        }
    }
}
impl Drop for Autosave {
    fn drop(&mut self) {
        self.stop_and_wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn autosave_debounces_and_has_a_maximum_wait() {
        let now = Instant::now();
        let mut state = SaveState::new(Config::default(), now);
        assert!(!state.due(now + Duration::from_millis(200), false));
        assert!(state.due(now + DEBOUNCE, false));
        state.saved = Some(state.seen.clone());
        assert!(!state.due(now + MAX_WAIT, false));
        for tick in 1..=25 {
            let mut config = Config::default();
            config.input_gain = tick as f32;
            state.observe(config, now + Duration::from_millis(tick * 100));
        }
        assert!(state.due(now + Duration::from_millis(2500), false));
        state.retry_at = Some(now + Duration::from_secs(10));
        assert!(!state.due(now + Duration::from_secs(3), false));
        assert!(state.due(now + Duration::from_secs(3), true));
    }
    #[test]
    fn background_writer_flushes_latest_state_on_normal_exit() {
        let path = std::env::temp_dir().join(format!(
            "kynn-persistence-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let value = Arc::new(Mutex::new(Config::default()));
        let sample = Arc::clone(&value);
        let saver = Autosave::start(path.clone(), move || sample.lock().unwrap().clone()).unwrap();
        value.lock().unwrap().clarity_percent = 42;
        saver.finish();
        assert_eq!(
            Config::load_from_path(path.to_str().unwrap())
                .unwrap()
                .clarity_percent,
            42
        );
        std::fs::remove_file(path).unwrap();
    }
}
