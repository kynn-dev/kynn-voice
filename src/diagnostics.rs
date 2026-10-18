//! Small, bounded local log retained for failures without a console.
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
#[derive(Clone)]
pub struct LogWriter(Arc<Mutex<LogFile>>);
struct LogFile {
    file: std::fs::File,
    path: std::path::PathBuf,
    bytes: u64,
}

impl LogWriter {
    pub fn open() -> io::Result<Self> {
        let dir = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Kynn Voice")
            .join("logs");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("kynn-voice.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(LogFile { file, path, bytes }))))
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut log = self
            .0
            .lock()
            .map_err(|_| io::Error::other("log mutex poisoned"))?;
        if log.bytes.saturating_add(buf.len() as u64) > MAX_LOG_BYTES {
            // One bounded previous log. No rotating loop or unbounded archive.
            log.file.flush()?;
            std::fs::copy(&log.path, log.path.with_extension("previous.log"))?;
            log.file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&log.path)?;
            log.bytes = 0;
        }
        log.file.write_all(buf)?;
        log.bytes += buf.len() as u64;
        // Headless diagnostics continue to be visible in captured stdout.
        let _ = io::stdout().write_all(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("log mutex poisoned"))?
            .file
            .flush()
    }
}
