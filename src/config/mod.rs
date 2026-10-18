pub mod cli;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::dsp::autogain::AutoGainSettings;
use crate::dsp::eq::EqSettings;
use crate::dsp::gate::GateSettings;

/// User-facing processing modes. Each mode uses exactly one neural denoiser.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ProcessingMode {
    /// Single-pass RNNoise with the lowest CPU cost.
    #[default]
    #[serde(rename = "light", alias = "rnnoise")]
    Light = 0,
    /// DeepFilterNet without the optional voice DSP chain.
    #[serde(rename = "balanced", alias = "deepfilter")]
    Balanced = 1,
    /// DeepFilterNet followed by the existing voice DSP chain.
    #[serde(rename = "studio")]
    Studio = 2,
}

impl ProcessingMode {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Balanced,
            2 => Self::Studio,
            _ => Self::Light,
        }
    }
}

impl std::fmt::Display for ProcessingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Light => write!(f, "Leve (RNNoise)"),
            Self::Balanced => write!(f, "Equilibrado (DeepFilterNet)"),
            Self::Studio => write!(f, "Estúdio (DeepFilterNet + voz)"),
        }
    }
}

/// Persisted configuration (TOML file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub virtual_device: Option<String>,
    pub denoise_enabled: bool,
    pub hard_mode: bool,
    /// `engine` is accepted as a legacy TOML key from Noise Gator 0.1.x.
    #[serde(alias = "engine")]
    pub mode: ProcessingMode,
    pub input_gain: f32,
    pub output_gain: f32,
    pub eq: EqSettings,
    pub gate: GateSettings,
    pub autogain: AutoGainSettings,
    pub suppression_level: f32,
    pub advanced_eq: bool,
    /// Last position of the native clarity control, independent of band edits.
    pub clarity_percent: u32,
    /// Desired audio state; absence of a microphone is not a user pause.
    pub start_processing: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            virtual_device: None,
            denoise_enabled: true,
            hard_mode: false,
            mode: ProcessingMode::default(),
            input_gain: 1.0,
            output_gain: 1.0,
            eq: EqSettings::default(),
            gate: GateSettings::default(),
            autogain: AutoGainSettings::default(),
            suppression_level: 0.75,
            advanced_eq: false,
            clarity_percent: 100,
            start_processing: true,
        }
    }
}

impl Config {
    /// Load config from a specific path.
    pub fn load_from_path(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::load_from(path.as_ref().to_path_buf())
    }

    fn load_from(path: PathBuf) -> Result<Self> {
        let read = |file: &std::path::Path| -> Result<Self> {
            Ok(toml::from_str(&std::fs::read_to_string(file)?)?)
        };
        match read(&path) {
            Ok(config) => Ok(config),
            Err(error) => match read(&path.with_extension("previous.toml")) {
                Ok(config) => {
                    tracing::warn!(
                        "Recovered previous valid settings after config read failure: {error}"
                    );
                    Ok(config)
                }
                Err(_) => Err(error),
            },
        }
    }

    /// Save config to an explicit path, matching the path accepted by `--config`.
    pub fn save_to_path(&self, path: impl Into<PathBuf>) -> Result<()> {
        self.save_to(path.into())
    }

    fn save_to(&self, path: PathBuf) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        // Keep the last valid primary. A corrupt primary must never replace a
        // valid recovery file. Both files are published by same-directory rename.
        if let Ok(previous) = std::fs::read_to_string(&path) {
            if toml::from_str::<Config>(&previous).is_ok() {
                atomic_write(&path.with_extension("previous.toml"), previous.as_bytes())?;
            }
        }
        atomic_write(&path, content.as_bytes())?;
        tracing::debug!("Config saved to {}", path.display());
        Ok(())
    }

    /// Config file path: `~/.config/noise-gator/config.toml` (or platform equivalent).
    pub fn path() -> PathBuf {
        dirs_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("config.toml")
    }
}

/// Flush the complete temporary file before publishing it. Never truncate the
/// live config, and clean up only the temporary file owned by this operation.
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Config needs a file name"))?;
    let mut temporary_name = name.to_os_string();
    temporary_name.push(format!(".{}.{sequence}.tmp", std::process::id()));
    let temporary = path.with_file_name(temporary_name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::Storage::FileSystem::{
                MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            };
            use windows::core::HSTRING;
            MoveFileExW(
                &HSTRING::from(temporary.as_os_str()),
                &HSTRING::from(path.as_os_str()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )?;
        }
        #[cfg(not(target_os = "windows"))]
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Platform-appropriate config directory.
fn dirs_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("noise-gator"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join("Library/Application Support/noise-gator"))
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.config")))
            .map(|p| PathBuf::from(p).join("noise-gator"))
    }
}

/// Atomic runtime settings shared between the audio thread and tray UI.
/// These are the "live" values that the pipeline reads on every callback.
pub struct RuntimeSettings {
    pub denoise_enabled: AtomicBool,
    pub hard_mode: AtomicBool,
    pub input_gain: AtomicU32,
    pub output_gain: AtomicU32,
    pub eq_bass_db10: std::sync::atomic::AtomicI32,
    pub eq_mid_db10: std::sync::atomic::AtomicI32,
    pub eq_treble_db10: std::sync::atomic::AtomicI32,
    pub eq_enabled: AtomicBool,
    // Gate settings (f32 stored as AtomicU32 bit patterns)
    pub gate_threshold: AtomicU32,
    pub gate_attack_ms: AtomicU32,
    pub gate_release_ms: AtomicU32,
    pub gate_hold_ms: AtomicU32,
    pub gate_floor: AtomicU32,
    // Auto-gain settings
    pub autogain_enabled: AtomicBool,
    pub autogain_target_rms: AtomicU32,
    pub autogain_max_gain: AtomicU32,
    // Processing mode selection
    pub mode: std::sync::atomic::AtomicU8,
    pub suppression_level: AtomicU32,
    // UI preference (not read by audio thread)
    pub advanced_eq: AtomicBool,
    pub clarity_percent: AtomicU32,
}

impl RuntimeSettings {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            denoise_enabled: AtomicBool::new(cfg.denoise_enabled),
            hard_mode: AtomicBool::new(cfg.hard_mode),
            input_gain: AtomicU32::new(cfg.input_gain.to_bits()),
            output_gain: AtomicU32::new(cfg.output_gain.to_bits()),
            eq_bass_db10: std::sync::atomic::AtomicI32::new((cfg.eq.bass_db * 10.0) as i32),
            eq_mid_db10: std::sync::atomic::AtomicI32::new((cfg.eq.mid_db * 10.0) as i32),
            eq_treble_db10: std::sync::atomic::AtomicI32::new((cfg.eq.treble_db * 10.0) as i32),
            eq_enabled: AtomicBool::new(cfg.eq.enabled),
            gate_threshold: AtomicU32::new(cfg.gate.threshold.to_bits()),
            gate_attack_ms: AtomicU32::new(cfg.gate.attack_ms.to_bits()),
            gate_release_ms: AtomicU32::new(cfg.gate.release_ms.to_bits()),
            gate_hold_ms: AtomicU32::new(cfg.gate.hold_ms.to_bits()),
            gate_floor: AtomicU32::new(cfg.gate.floor.to_bits()),
            autogain_enabled: AtomicBool::new(cfg.autogain.enabled),
            autogain_target_rms: AtomicU32::new(cfg.autogain.target_rms.to_bits()),
            autogain_max_gain: AtomicU32::new(cfg.autogain.max_gain.to_bits()),
            mode: std::sync::atomic::AtomicU8::new(cfg.mode.as_u8()),
            suppression_level: AtomicU32::new(cfg.suppression_level.to_bits()),
            advanced_eq: AtomicBool::new(cfg.advanced_eq),
            clarity_percent: AtomicU32::new(cfg.clarity_percent.min(100)),
        }
    }

    pub fn load_mode(&self) -> ProcessingMode {
        ProcessingMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    /// Read current gate settings from atomics.
    pub fn load_gate_settings(&self) -> GateSettings {
        GateSettings {
            threshold: f32::from_bits(self.gate_threshold.load(Ordering::Relaxed)),
            attack_ms: f32::from_bits(self.gate_attack_ms.load(Ordering::Relaxed)),
            release_ms: f32::from_bits(self.gate_release_ms.load(Ordering::Relaxed)),
            hold_ms: f32::from_bits(self.gate_hold_ms.load(Ordering::Relaxed)),
            floor: f32::from_bits(self.gate_floor.load(Ordering::Relaxed)),
        }
    }

    /// Read current autogain settings from atomics.
    pub fn load_autogain_settings(&self) -> AutoGainSettings {
        let mut s = AutoGainSettings::default();
        s.enabled = self.autogain_enabled.load(Ordering::Relaxed);
        s.target_rms = f32::from_bits(self.autogain_target_rms.load(Ordering::Relaxed));
        s.max_gain = f32::from_bits(self.autogain_max_gain.load(Ordering::Relaxed));
        s
    }

    /// Read current EQ settings from atomics.
    pub fn load_eq_settings(&self) -> EqSettings {
        EqSettings {
            bass_db: self.eq_bass_db10.load(Ordering::Relaxed) as f32 / 10.0,
            mid_db: self.eq_mid_db10.load(Ordering::Relaxed) as f32 / 10.0,
            treble_db: self.eq_treble_db10.load(Ordering::Relaxed) as f32 / 10.0,
            enabled: self.eq_enabled.load(Ordering::Relaxed),
        }
    }

    /// Snapshot current runtime state back into a Config for saving.
    pub fn to_config(&self, base: &Config) -> Config {
        let mut autogain = base.autogain;
        autogain.enabled = self.autogain_enabled.load(Ordering::Relaxed);
        autogain.target_rms = f32::from_bits(self.autogain_target_rms.load(Ordering::Relaxed));
        autogain.max_gain = f32::from_bits(self.autogain_max_gain.load(Ordering::Relaxed));
        Config {
            input_device: base.input_device.clone(),
            output_device: base.output_device.clone(),
            virtual_device: base.virtual_device.clone(),
            denoise_enabled: self.denoise_enabled.load(Ordering::Relaxed),
            hard_mode: self.hard_mode.load(Ordering::Relaxed),
            mode: self.load_mode(),
            input_gain: f32::from_bits(self.input_gain.load(Ordering::Relaxed)),
            output_gain: f32::from_bits(self.output_gain.load(Ordering::Relaxed)),
            eq: self.load_eq_settings(),
            gate: self.load_gate_settings(),
            autogain,
            suppression_level: f32::from_bits(self.suppression_level.load(Ordering::Relaxed)),
            advanced_eq: self.advanced_eq.load(Ordering::Relaxed),
            clarity_percent: self.clarity_percent.load(Ordering::Relaxed),
            start_processing: base.start_processing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ProcessingMode, RuntimeSettings};

    fn test_path() -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kynn-config-{}-{unique}.toml", std::process::id()))
    }

    #[test]
    fn processing_modes_round_trip_as_atomic_values() {
        for mode in [
            ProcessingMode::Light,
            ProcessingMode::Balanced,
            ProcessingMode::Studio,
        ] {
            assert_eq!(ProcessingMode::from_u8(mode.as_u8()), mode);
        }
    }

    #[test]
    fn legacy_engine_settings_are_mapped_to_modes() {
        let balanced: Config = toml::from_str("engine = 'deepfilter'").unwrap();
        let light: Config = toml::from_str("engine = 'rnnoise'").unwrap();
        assert_eq!(balanced.mode, ProcessingMode::Balanced);
        assert_eq!(light.mode, ProcessingMode::Light);
    }

    #[test]
    fn mode_settings_are_persisted_with_new_names() {
        let studio: Config = toml::from_str("mode = 'studio'").unwrap();
        assert_eq!(studio.mode, ProcessingMode::Studio);
        let serialized = toml::to_string(&studio).unwrap();
        assert!(serialized.contains("mode = \"studio\""));
        assert!(!serialized.contains("engine ="));
    }

    #[test]
    fn legacy_config_defaults_new_preferences_without_changing_audio() {
        let config: Config = toml::from_str("mode='balanced'\nsuppression_level=0.37").unwrap();
        assert!(config.start_processing);
        assert_eq!(config.clarity_percent, 100);
        assert_eq!(config.mode, ProcessingMode::Balanced);
        assert_eq!(config.suppression_level, 0.37);
    }

    #[test]
    fn all_runtime_choices_round_trip_including_custom_autogain_fields() {
        let mut config = Config::default();
        config.mode = ProcessingMode::Studio;
        config.start_processing = false;
        config.input_device = Some("wasapi:input-test".into());
        config.output_device = Some("wasapi:monitor-test".into());
        config.virtual_device = Some("wasapi:virtual-test".into());
        config.denoise_enabled = false;
        config.clarity_percent = 42;
        config.input_gain = 1.55;
        config.output_gain = 0.65;
        config.suppression_level = 0.37;
        config.eq.bass_db = -3.7;
        config.eq.mid_db = 2.8;
        config.eq.treble_db = 1.9;
        config.autogain.min_gain = 1.5;
        config.autogain.rms_smoothing = 0.875;
        config.autogain.enabled = true;
        let snapshot = RuntimeSettings::from_config(&config).to_config(&config);
        assert_eq!(snapshot, config);
        let decoded: Config = toml::from_str(&toml::to_string_pretty(&snapshot).unwrap()).unwrap();
        assert_eq!(decoded, config);
    }

    #[test]
    fn atomic_config_replace_keeps_previous_valid_version_and_recovers_it() {
        let path = test_path();
        let previous = Config::default();
        previous.save_to_path(&path).unwrap();
        let mut latest = previous.clone();
        latest.mode = ProcessingMode::Studio;
        latest.save_to_path(&path).unwrap();
        assert_eq!(Config::load_from_path(&path).unwrap(), latest);
        std::fs::write(&path, "mode = \"interrupted").unwrap();
        let recovered = Config::load_from_path(&path).unwrap();
        assert_eq!(recovered, previous);
        recovered.save_to_path(&path).unwrap();
        assert_eq!(Config::load_from_path(&path).unwrap(), previous);
        assert_eq!(
            Config::load_from_path(path.with_extension("previous.toml")).unwrap(),
            previous
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(path.with_extension("previous.toml")).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn failed_atomic_replace_preserves_primary_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = test_path();
        let config = Config::default();
        config.save_to_path(&path).unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(super::atomic_write(&path, b"mode='studio'").is_err());
        assert_eq!(Config::load_from_path(&path).unwrap(), config);
        drop(locked);
        std::fs::remove_file(path).unwrap();
    }
}
