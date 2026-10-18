//! Opt-in, per-user Windows startup registration. Registry I/O stays off the UI.

use std::path::PathBuf;
static INITIALIZATION_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[derive(Clone, Debug)]
pub struct StartupSnapshot {
    pub supported: bool,
    pub ready: bool,
    pub enabled: bool,
    pub error: Option<String>,
}

impl Default for StartupSnapshot {
    fn default() -> Self {
        Self {
            supported: cfg!(windows),
            ready: false,
            enabled: false,
            error: None,
        }
    }
}

/// Read the registry asynchronously. This never enables or repairs registration.
pub fn initialize(config_path: PathBuf) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let result = platform::initialize(config_path);
        *INITIALIZATION_ERROR
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            result.as_ref().err().map(|error| format!("{error:#}"));
        result
    }
    #[cfg(not(windows))]
    {
        let _ = config_path;
        anyhow::bail!("Starting with Windows is not supported on this platform")
    }
}

/// Reading this before initialize creates neither a worker nor a registry key.
pub fn snapshot() -> StartupSnapshot {
    #[cfg(windows)]
    {
        if let Some(error) = INITIALIZATION_ERROR
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return StartupSnapshot {
                ready: true,
                error: Some(error),
                ..StartupSnapshot::default()
            };
        }
        platform::snapshot()
    }
    #[cfg(not(windows))]
    {
        StartupSnapshot::default()
    }
}

/// Queue one desired state, replacing a pending request rather than accumulating.
pub fn request_enabled(enabled: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        platform::request_enabled(enabled)
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        anyhow::bail!("Starting with Windows is not supported on this platform")
    }
}

/// Wait for queued registration and read-back before normal application exit.
/// Registry I/O remains on the worker; no initialized worker is a no-op.
pub fn flush() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        platform::flush()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

#[cfg(any(windows, test))]
struct Shared {
    snapshot: StartupSnapshot,
    pending: Option<bool>,
}

#[cfg(any(windows, test))]
fn wait_for_completion(
    shared: &(std::sync::Mutex<Shared>, std::sync::Condvar),
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let state = shared
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("Startup registration worker state was poisoned"))?;
    let (state, _) = shared
        .1
        .wait_timeout_while(state, timeout.saturating_sub(started.elapsed()), |state| {
            state.pending.is_some() || !state.snapshot.ready
        })
        .map_err(|_| anyhow::anyhow!("Startup registration worker state was poisoned"))?;
    anyhow::ensure!(
        state.pending.is_none() && state.snapshot.ready,
        "Timed out waiting for Windows startup registration to finish and confirm its saved state"
    );
    if let Some(error) = &state.snapshot.error {
        anyhow::bail!("Windows startup registration could not be confirmed: {error}");
    }
    Ok(())
}

#[cfg(any(windows, test))]
const MAX_RUN_COMMAND_UTF16: usize = 260;

#[cfg(any(windows, test))]
fn quote_argument(argument: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        !argument.contains(['\0', '"']),
        "Startup paths must not contain NUL or quotation marks"
    );
    // No embedded quotes are accepted in a Windows path. Only trailing slashes
    // need doubling, so they cannot escape the closing quote in CommandLineToArgv.
    let trailing_slashes = argument.chars().rev().take_while(|c| *c == '\\').count();
    Ok(format!("\"{}{}\"", argument, "\\".repeat(trailing_slashes)))
}

#[cfg(any(windows, test))]
fn build_command(executable: &std::path::Path, config: &std::path::Path) -> anyhow::Result<String> {
    anyhow::ensure!(
        executable.is_absolute() && config.is_absolute(),
        "Startup executable and configuration paths must be absolute"
    );
    let executable = executable
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Startup executable path is not valid Unicode"))?;
    let config = config
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Startup configuration path is not valid Unicode"))?;
    let command = format!(
        "{} --config {}",
        quote_argument(executable)?,
        quote_argument(config)?
    );
    // Include the terminating NUL in the conservative Run-key character budget.
    anyhow::ensure!(
        command.encode_utf16().count() < MAX_RUN_COMMAND_UTF16,
        "Startup command exceeds the 260 UTF-16 unit limit; move the application or configuration to a shorter path"
    );
    Ok(command)
}

#[cfg(windows)]
mod platform {
    use super::{Shared, StartupSnapshot, build_command, wait_for_completion};
    use anyhow::Context;
    use std::path::PathBuf;
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    use std::time::Duration;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Kynn Voice";
    static SERVICE: OnceLock<Service> = OnceLock::new();
    static INITIALIZING: Mutex<()> = Mutex::new(());

    struct Service {
        command: String,
        shared: Arc<(Mutex<Shared>, Condvar)>,
    }

    pub(super) fn initialize(config: PathBuf) -> anyhow::Result<()> {
        let command = build_command(
            &std::env::current_exe().context("Locate executable for Windows startup")?,
            &config,
        )?;
        let _initializing = INITIALIZING
            .lock()
            .map_err(|_| anyhow::anyhow!("Startup initialization lock was poisoned"))?;
        if let Some(existing) = SERVICE.get() {
            anyhow::ensure!(
                existing.command == command,
                "Windows startup was already initialized for a different configuration"
            );
            return Ok(());
        }
        let shared = Arc::new((
            Mutex::new(Shared {
                snapshot: StartupSnapshot::default(),
                pending: None,
            }),
            Condvar::new(),
        ));
        let worker_state = Arc::clone(&shared);
        let worker_command = command.clone();
        std::thread::Builder::new()
            .name("windows-startup".into())
            .spawn(move || worker(worker_state, worker_command))
            .context("Start Windows startup registration worker")?;
        SERVICE
            .set(Service { command, shared })
            .map_err(|_| anyhow::anyhow!("Windows startup was initialized concurrently"))?;
        Ok(())
    }

    pub(super) fn snapshot() -> StartupSnapshot {
        let Some(service) = SERVICE.get() else {
            return StartupSnapshot::default();
        };
        match service.shared.0.lock() {
            Ok(shared) => shared.snapshot.clone(),
            Err(_) => StartupSnapshot {
                ready: true,
                error: Some("Startup registration worker state was poisoned".into()),
                ..StartupSnapshot::default()
            },
        }
    }

    pub(super) fn request_enabled(enabled: bool) -> anyhow::Result<()> {
        let service = SERVICE
            .get()
            .context("Windows startup has not been initialized")?;
        let mut shared = service
            .shared
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Startup registration worker state was poisoned"))?;
        shared.pending = Some(enabled);
        shared.snapshot.ready = false;
        shared.snapshot.error = None;
        // Both the worker and a shutdown flush may be waiting on this condvar.
        service.shared.1.notify_all();
        Ok(())
    }

    pub(super) fn flush() -> anyhow::Result<()> {
        match SERVICE.get() {
            Some(service) => wait_for_completion(&service.shared, Duration::from_secs(2)),
            None => Ok(()),
        }
    }

    fn worker(shared: Arc<(Mutex<Shared>, Condvar)>, command: String) {
        let mut action = None;
        loop {
            // The initial pass and periodic refresh only read. An existing Run
            // entry is never restored or rewritten without a user request.
            let mutation_error = action.and_then(|enabled| {
                registry::write_enabled(RUN_KEY, VALUE_NAME, &command, enabled)
                    .err()
                    .map(|error| format!("Unable to update Windows startup: {error:#}"))
            });
            let observed = registry::read(RUN_KEY, VALUE_NAME);
            let Ok(mut state) = shared.0.lock() else {
                return;
            };
            let mut error = mutation_error;
            match observed {
                Ok(value) => {
                    state.snapshot.enabled = value.is_some();
                    if error.is_none() {
                        error = match action {
                            Some(true) if value.as_deref() != Some(command.as_str()) => {
                                Some("Windows startup registration did not match after saving".into())
                            }
                            Some(false) if value.is_some() => {
                                Some("Windows startup registration still exists after removal".into())
                            }
                            None if value.as_deref().is_some_and(|value| value != command) => Some(
                                "Existing startup entry uses another executable or configuration; turn startup off and on to update it".into(),
                            ),
                            _ => None,
                        };
                    }
                }
                Err(read_error) => {
                    error = Some(match error {
                        Some(error) => format!("{error}; read-back failed: {read_error:#}"),
                        None => format!("Unable to read Windows startup: {read_error:#}"),
                    });
                }
            }
            // Keep failed actions visible until the user retries; periodic
            // successful reads must not mask a failed write or removal.
            if action.is_some() || error.is_some() || state.snapshot.error.is_none() {
                state.snapshot.error = error;
            }
            state.snapshot.ready = state.pending.is_none();
            shared.1.notify_all();
            while state.pending.is_none() {
                let Ok((next, timeout)) = shared.1.wait_timeout(state, Duration::from_secs(2))
                else {
                    return;
                };
                state = next;
                if timeout.timed_out() {
                    break;
                }
            }
            action = state.pending.take();
            if action.is_some() {
                state.snapshot.ready = false;
            }
        }
    }

    mod registry {
        use anyhow::Context;
        use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
            REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
            RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        };
        use windows::core::PCWSTR;

        // Only owned opened/created keys are closed. HKEY_CURRENT_USER is borrowed.
        struct Key(HKEY);

        impl Drop for Key {
            fn drop(&mut self) {
                unsafe {
                    let _ = RegCloseKey(self.0);
                }
            }
        }

        fn wide(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(Some(0)).collect()
        }

        fn open(path: &str, access: REG_SAM_FLAGS) -> anyhow::Result<Option<Key>> {
            let path = wide(path);
            let mut key = HKEY::default();
            let status = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(path.as_ptr()),
                    None,
                    access,
                    &mut key,
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            status
                .ok()
                .context("Open current-user startup registry key")?;
            Ok(Some(Key(key)))
        }

        fn create(path: &str) -> anyhow::Result<Key> {
            let path = wide(path);
            let mut key = HKEY::default();
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(path.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    None,
                    &mut key,
                    None,
                )
                .ok()
                .context("Create current-user startup registry key")?;
            }
            Ok(Key(key))
        }

        pub(super) fn read(path: &str, name: &str) -> anyhow::Result<Option<String>> {
            let Some(key) = open(path, KEY_QUERY_VALUE)? else {
                return Ok(None);
            };
            let name = wide(name);
            // Fixed-size storage avoids allocating from an untrusted registry
            // length. The Run command is limited to 260 UTF-16 units including NUL.
            let mut data = [0u16; super::super::MAX_RUN_COMMAND_UTF16];
            let mut bytes = std::mem::size_of_val(&data) as u32;
            let mut kind = REG_VALUE_TYPE::default();
            let status = unsafe {
                RegQueryValueExW(
                    key.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    Some(&mut kind),
                    Some(data.as_mut_ptr().cast()),
                    Some(&mut bytes),
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            status
                .ok()
                .context("Read current-user startup registry value")?;
            anyhow::ensure!(
                kind == REG_SZ,
                "Startup registry value must have type REG_SZ"
            );
            anyhow::ensure!(
                bytes >= 2 && bytes % 2 == 0,
                "Invalid startup registry string length"
            );
            let count = bytes as usize / 2;
            anyhow::ensure!(
                data[count - 1] == 0,
                "Startup registry string is not NUL terminated"
            );
            let command = String::from_utf16(&data[..count - 1])
                .context("Startup registry command is not valid Unicode")?;
            anyhow::ensure!(
                !command.is_empty() && !command.contains('\0'),
                "Startup registry command is empty or contains an embedded NUL"
            );
            Ok(Some(command))
        }

        pub(super) fn write_enabled(
            path: &str,
            name: &str,
            command: &str,
            enabled: bool,
        ) -> anyhow::Result<()> {
            let name = wide(name);
            if enabled {
                let key = create(path)?;
                let command = wide(command);
                let bytes: Vec<u8> = command.iter().flat_map(|unit| unit.to_le_bytes()).collect();
                unsafe {
                    RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes))
                        .ok()
                        .context("Write current-user startup registry value")?;
                }
            } else if let Some(key) = open(path, KEY_SET_VALUE)? {
                let status = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
                if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                    status
                        .ok()
                        .context("Delete current-user startup registry value")?;
                }
            }
            Ok(())
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use std::time::{SystemTime, UNIX_EPOCH};
            use windows::Win32::System::Registry::{REG_DWORD, RegDeleteKeyW};

            struct TestKey(String);

            impl Drop for TestKey {
                fn drop(&mut self) {
                    let path = wide(&self.0);
                    // Exact test key only, never a recursive delete or Run key.
                    let result = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
                    if result != ERROR_SUCCESS && result != ERROR_FILE_NOT_FOUND {
                        eprintln!("Unable to remove test registry key: {result:?}");
                    }
                }
            }

            #[test]
            #[ignore = "Writes only an isolated HKCU Software\\Kynn Voice\\Tests subkey"]
            fn registry_round_trip_in_isolated_test_key() -> anyhow::Result<()> {
                let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
                let test_key = TestKey(format!(
                    r"Software\Kynn Voice\Tests\startup-{}-{unique}",
                    std::process::id()
                ));
                let name = "Kynn Voice";
                let command =
                    r#""C:\Kynn Voice\kynn-voice.exe" --config "C:\Usuários\ação\config.toml""#;
                assert_eq!(read(&test_key.0, name)?, None);
                write_enabled(&test_key.0, name, command, true)?;
                assert_eq!(read(&test_key.0, name)?.as_deref(), Some(command));
                {
                    let key = create(&test_key.0)?;
                    let name = wide(name);
                    unsafe {
                        RegSetValueExW(
                            key.0,
                            PCWSTR(name.as_ptr()),
                            None,
                            REG_DWORD,
                            Some(&1u32.to_le_bytes()),
                        )
                        .ok()?;
                    }
                }
                assert!(
                    read(&test_key.0, name).is_err(),
                    "REG_DWORD must not appear enabled"
                );
                write_enabled(&test_key.0, name, command, false)?;
                assert_eq!(read(&test_key.0, name)?, None);
                write_enabled(&test_key.0, name, command, false)?;
                // Explicit cleanup is asserted as well as being attempted on panic.
                let path = wide(&test_key.0);
                unsafe {
                    RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())).ok()?;
                }
                assert!(open(&test_key.0, KEY_QUERY_VALUE)?.is_none());
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn absolute(value: &str) -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from(format!(r"C:\{value}"))
        }
        #[cfg(not(windows))]
        {
            PathBuf::from(format!("/{value}"))
        }
    }

    #[test]
    fn command_quotes_spaces_and_unicode_and_only_expected_arguments() {
        let executable = absolute("Kynn Voice/kynn-voice.exe");
        let config = absolute("Usuários/ação/config.toml");
        let command = build_command(&executable, &config).unwrap();
        assert_eq!(
            command,
            format!(
                "\"{}\" --config \"{}\"",
                executable.display(),
                config.display()
            )
        );
        assert!(!command.contains("--settings"));
    }

    #[test]
    fn quote_doubles_trailing_backslashes() {
        assert_eq!(
            quote_argument(r"C:\directory\").unwrap(),
            "\"C:\\directory\\\\\""
        );
        assert_eq!(
            quote_argument(r"C:\directory\\").unwrap(),
            "\"C:\\directory\\\\\\\\\""
        );
    }

    #[test]
    fn command_rejects_relative_paths_nul_and_quotes() {
        let absolute = absolute("kynn-voice.exe");
        assert!(build_command(Path::new("relative.exe"), &absolute).is_err());
        assert!(build_command(&absolute, Path::new("relative.toml")).is_err());
        assert!(quote_argument("C:\\invalid\0path").is_err());
        assert!(quote_argument("C:\\invalid\"path").is_err());
    }

    #[test]
    fn command_enforces_utf16_limit_including_terminator() {
        let executable = absolute("k.exe");
        let overhead = build_command(&executable, &absolute("config-"))
            .unwrap()
            .encode_utf16()
            .count();
        let remaining = MAX_RUN_COMMAND_UTF16 - 1 - overhead;
        let at_limit = build_command(
            &executable,
            &absolute(&format!("config-{}", "a".repeat(remaining))),
        )
        .unwrap();
        assert_eq!(at_limit.encode_utf16().count() + 1, MAX_RUN_COMMAND_UTF16);
        assert!(
            build_command(
                &executable,
                &absolute(&format!("config-{}", "a".repeat(remaining + 1)))
            )
            .is_err()
        );
        // Astral Unicode takes two UTF-16 units rather than one Unicode scalar.
        assert!(
            build_command(
                &executable,
                &absolute(&format!("config-{}", "😀".repeat(remaining)))
            )
            .is_err()
        );
    }

    #[test]
    fn completion_wait_observes_readback_without_losing_notification() {
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::Duration;

        let shared = Arc::new((
            Mutex::new(Shared {
                snapshot: StartupSnapshot::default(),
                pending: Some(true),
            }),
            Condvar::new(),
        ));
        let completion = Arc::clone(&shared);
        let worker = std::thread::spawn(move || {
            let mut state = completion.0.lock().unwrap();
            state.pending = None;
            state.snapshot.enabled = true;
            state.snapshot.ready = true;
            completion.1.notify_all();
        });
        // Also succeeds if the worker notified before this starts waiting.
        wait_for_completion(&shared, Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        wait_for_completion(&shared, Duration::ZERO).unwrap();
    }

    #[test]
    fn completion_wait_times_out_and_reports_operation_error() {
        use std::sync::{Condvar, Mutex};
        use std::time::Duration;

        let shared = (
            Mutex::new(Shared {
                snapshot: StartupSnapshot::default(),
                pending: Some(true),
            }),
            Condvar::new(),
        );
        let error = wait_for_completion(&shared, Duration::from_millis(5)).unwrap_err();
        assert!(error.to_string().contains("Timed out"));
        {
            let mut state = shared.0.lock().unwrap();
            state.pending = None;
            state.snapshot.ready = true;
            state.snapshot.error = Some("Access denied while saving".into());
        }
        let error = wait_for_completion(&shared, Duration::ZERO).unwrap_err();
        assert!(error.to_string().contains("Access denied while saving"));
    }
}
