#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod audio;
mod config;
mod diagnostics;
mod dsp;
mod models;
mod persistence;
mod resources;
mod startup;
mod tray;
mod ui;

use std::sync::Arc;

use audio::device;
use audio::pipeline::{AUDIO_METRICS, Pipeline};
use config::cli::Cli;
use config::{Config, ProcessingMode, RuntimeSettings};

fn main() {
    // Parse CLI first (before logging — --list-devices should be clean)
    let cli = Cli::parse();

    // Init logging
    let log = diagnostics::LogWriter::open().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("kynn_voice=info".parse().unwrap()),
        )
        .with_ansi(false)
        .with_writer(move || -> Box<dyn std::io::Write> {
            match &log {
                Some(log) => Box::new(log.clone()),
                None => Box::new(std::io::stdout()),
            }
        })
        .init();

    // ── List devices mode ───────────────────────────────────────────
    if cli.list_devices {
        print_devices();
        return;
    }

    let _resource_guard = match resources::ResourceGuard::install() {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!("Cannot enforce process resource limits: {error}");
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
                use windows::core::HSTRING;
                MessageBoxW(
                    None,
                    &HSTRING::from(format!(
                        "Não foi possível ativar os limites de CPU/memória. Kynn Voice não iniciou o áudio.\n{error}"
                    )),
                    &HSTRING::from("Kynn Voice"),
                    MB_OK | MB_ICONERROR,
                );
            }
            std::process::exit(1);
        }
    };

    if cli.resource_self_test {
        #[cfg(target_os = "windows")]
        if let Err(error) = resources::self_test(&_resource_guard) {
            eprintln!("Resource self test failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if cli.ui_smoke_test {
        #[cfg(target_os = "windows")]
        if let Err(error) = ui::window::smoke_test() {
            eprintln!("UI smoke test failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    // ── Single instance guard ──────────────────────────────────────
    let _instance_guard = enforce_single_instance();

    // ── Load config (CLI overrides file) ────────────────────────────
    let config_path = match std::path::absolute(
        cli.config
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(Config::path),
    ) {
        Ok(path) => path,
        Err(error) => {
            tracing::error!("Cannot resolve configuration path: {error}");
            std::process::exit(1);
        }
    };
    let mut config = {
        match config::Config::load_from_path(&config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to load config from {}: {e}", config_path.display());
                Config::default()
            }
        }
    };
    if let Err(error) = startup::initialize(config_path.clone()) {
        tracing::warn!("Windows startup option unavailable: {error}");
    }

    // CLI overrides
    if cli.input.is_some() {
        config.input_device = cli.input;
    }
    if cli.monitor.is_some() {
        config.output_device = cli.monitor;
    }
    if cli.virtual_device.is_some() {
        config.virtual_device = cli.virtual_device;
    }
    if cli.no_denoise {
        config.denoise_enabled = false;
    }
    if cli.hard_mode {
        config.hard_mode = true;
    }
    if let Some(ref mode) = cli.mode {
        match parse_mode(mode) {
            Some(mode) => config.mode = mode,
            None => tracing::warn!("Unknown mode '{mode}', using the saved mode"),
        }
    } else if let Some(ref engine) = cli.engine {
        tracing::warn!("--engine is deprecated; use --mode");
        match engine.to_lowercase().as_str() {
            "deepfilter" | "deep-filter" | "df" => config.mode = ProcessingMode::Balanced,
            "rnnoise" | "rnn" => config.mode = ProcessingMode::Light,
            other => tracing::warn!("Unknown legacy engine '{other}', using the saved mode"),
        }
    }

    let config_base = config.clone();

    // ── Build pipeline ──────────────────────────────────────────────
    let settings = Arc::new(RuntimeSettings::from_config(&config));
    let pipeline = Arc::new(Pipeline::new(Arc::clone(&settings)));

    // Auto-start. If the selected USB input is not available yet, keep the app
    // alive and let the watchdog retry the configured device without silently
    // routing another microphone.
    if config.start_processing {
        if let Err(e) = pipeline.start(
            config.input_device.as_deref(),
            config.output_device.as_deref(),
            config.virtual_device.as_deref(),
        ) {
            tracing::error!("Audio pipeline is waiting for the configured devices: {e}");
            eprintln!("Audio pipeline is waiting for the configured devices: {e}");
        }
    } else {
        pipeline.remember_devices(
            config.input_device.as_deref(),
            config.output_device.as_deref(),
            config.virtual_device.as_deref(),
        );
        tracing::info!("Restored saved paused audio state");
    }

    let saved_pipeline = Arc::clone(&pipeline);
    let saved_base = config_base.clone();
    let autosave = match persistence::Autosave::start(config_path, move || {
        saved_pipeline.to_config(&saved_base)
    }) {
        Ok(saver) => saver,
        Err(error) => {
            tracing::error!("Cannot start settings persistence: {error}");
            std::process::exit(1);
        }
    };

    // ── Start device watchdog for auto-reconnect ─────────────────
    let _watchdog = Pipeline::start_watchdog(Arc::clone(&pipeline));

    // ── Run mode ────────────────────────────────────────────────────
    if cli.headless {
        run_headless();
    } else if let Err(e) = tray::run(Arc::clone(&pipeline), config, cli.open_settings) {
        tracing::error!("Tray error: {e}");
        eprintln!("Tray error: {e}");
    }

    // Cleanup
    // Flush before stop() changes the desired state. Quit preserves whether the
    // user had audio enabled; an explicit Stop remains paused next launch.
    if let Err(error) = startup::flush() {
        tracing::error!("Could not finish Windows startup change: {error}");
    }
    autosave.finish();
    drop(_watchdog);
    pipeline.stop();
}

fn print_devices() {
    match device::audio_host() {
        Ok(host) => println!("Audio backend: {}", host.id()),
        Err(e) => eprintln!("Audio backend error: {e}"),
    }
    println!("=== Input Devices ===");
    match device::list_input_devices() {
        Ok(devices) => {
            for d in &devices {
                println!("  {}", d.display_name());
                println!("    ID: {}", d.id);
            }
        }
        Err(e) => eprintln!("  Error: {e}"),
    }

    println!("\n=== Output Devices ===");
    match device::list_output_devices() {
        Ok(devices) => {
            for d in &devices {
                println!("  {}", d.display_name());
                println!("    ID: {}", d.id);
            }
        }
        Err(e) => eprintln!("  Error: {e}"),
    }

    if let Some(virt) = device::detect_virtual_device() {
        println!("\n=== Virtual Device ===");
        let label = device::list_output_devices()
            .ok()
            .and_then(|devices| devices.into_iter().find(|device| device.id == virt))
            .map(|device| device.friendly_name)
            .unwrap_or_else(|| "VB-CABLE".to_string());
        println!("  {label}");
        println!("    ID: {virt}");
    }
}

fn parse_mode(value: &str) -> Option<ProcessingMode> {
    match value.to_ascii_lowercase().as_str() {
        "light" | "rnnoise" | "rnn" => Some(ProcessingMode::Light),
        "balanced" | "deepfilter" | "deep-filter" | "df" => Some(ProcessingMode::Balanced),
        "studio" => Some(ProcessingMode::Studio),
        _ => None,
    }
}

fn run_headless() {
    tracing::info!("Running headless. Press Ctrl+C to stop.");

    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = Arc::clone(&running);

    ctrlc_handler(move || {
        r.store(false, std::sync::atomic::Ordering::Relaxed);
    });

    let mut previous_report = std::time::Instant::now();
    let mut previous_frames = AUDIO_METRICS
        .processed_frames
        .load(std::sync::atomic::Ordering::Relaxed);
    while running.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let now = std::time::Instant::now();
        if now.duration_since(previous_report) < std::time::Duration::from_secs(1) {
            continue;
        }

        let processed_frames = AUDIO_METRICS
            .processed_frames
            .load(std::sync::atomic::Ordering::Relaxed);
        tracing::info!(
            "Audio metrics: frames/s={} total={} frame_us(last/avg/max)={}/{}/{} cpu_estimate_single_core={:.1}% latency_us(last/avg/max)={}/{}/{} input_overrun_frames={} output_underrun_frames={} output_overrun_samples={} output_callback_max_frames={}",
            processed_frames.saturating_sub(previous_frames),
            processed_frames,
            AUDIO_METRICS
                .frame_last_us
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .frame_average_us
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .frame_max_us
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .dsp_cpu_percent_x10
                .load(std::sync::atomic::Ordering::Relaxed) as f64
                / 10.0,
            AUDIO_METRICS
                .latency_last_us
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .latency_average_us
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .latency_max_us
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .input_overrun_frames
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .output_underrun_frames
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .output_overrun_samples
                .load(std::sync::atomic::Ordering::Relaxed),
            AUDIO_METRICS
                .output_callback_frames_max
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        previous_frames = processed_frames;
        previous_report = now;
    }
}

fn ctrlc_handler<F: FnOnce() + Send + 'static>(f: F) {
    let f = std::sync::Mutex::new(Some(f));
    let _ = ctrlc::set_handler(move || {
        if let Some(f) = f.lock().unwrap().take() {
            f();
        }
    });
}

/// Ensures only one instance of Kynn Voice runs at a time.
/// Returns a guard that holds the OS mutex — when dropped (or process exits),
/// the mutex is released so a new instance can start.
#[cfg(target_os = "windows")]
fn enforce_single_instance() -> windows::Win32::Foundation::HANDLE {
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;

    let name = HSTRING::from("Global\\KynnVoice_SingleInstance");
    let handle = unsafe { CreateMutexW(None, true, &name) };

    match handle {
        Ok(h) => {
            // If ERROR_ALREADY_EXISTS, another instance owns the mutex
            let last_err = unsafe { windows::Win32::Foundation::GetLastError() };
            if last_err == windows::Win32::Foundation::ERROR_ALREADY_EXISTS {
                eprintln!("Kynn Voice is already running.");
                std::process::exit(0);
            }
            h
        }
        Err(_) => {
            // CreateMutex failed — another instance likely running, or access denied
            eprintln!("Kynn Voice is already running.");
            std::process::exit(0);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn enforce_single_instance() -> std::fs::File {
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::io::AsRawFd;

    let lock_path = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string())
        + "/noise-gator.lock";

    // Open-or-create the lock file (not truncated — flock is on the fd, not content)
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap_or_else(|e| {
            eprintln!("Cannot open lock file {lock_path}: {e}");
            std::process::exit(1);
        });

    // Non-blocking exclusive lock — fails immediately if another instance holds it
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        eprintln!("Kynn Voice is already running.");
        std::process::exit(0);
    }

    // Write PID for diagnostics (optional, doesn't affect locking)
    let mut f = &file;
    let _ = f.write_all(format!("{}", std::process::id()).as_bytes());

    // Return the File — the lock is held as long as this fd is open.
    // When the process exits (even on crash/SIGKILL), the OS releases the flock.
    file
}
