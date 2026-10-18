use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{
    BufferSize, Device, DeviceDescription, Host, InterfaceType, SampleFormat, SupportedBufferSize,
    SupportedStreamConfig,
};

/// Known virtual audio device name patterns (checked in priority order).
const VIRTUAL_DEVICE_PATTERNS: &[&str] = &[
    "CABLE Input",
    "VB-Audio Virtual Cable",
    "VB-Cable",
    "Kynn Voice",
    "NoiseGator",
];
const VB_CABLE_PATTERNS: &[&str] = &["CABLE Input", "VB-Audio Virtual Cable", "VB-Cable"];

/// Audio device descriptor, keyed by the backend's stable endpoint ID.
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: String,
    pub friendly_name: String,
    pub interface_type: InterfaceType,
    pub is_default: bool,
}

/// Return the audio host selected by CPAL. On Windows the application is
/// deliberately restricted to WASAPI so device selection and stream behavior
/// are predictable for the system-wide virtual microphone path.
pub fn audio_host() -> Result<Host> {
    let host = cpal::default_host();
    #[cfg(target_os = "windows")]
    if !host.id().to_string().eq_ignore_ascii_case("wasapi") {
        return Err(anyhow!(
            "Expected the WASAPI audio host on Windows, got '{}'",
            host.id()
        ));
    }
    Ok(host)
}

impl AudioDevice {
    /// Disambiguated name for UI display.
    pub fn display_name(&self) -> String {
        let interface = if self.interface_type == InterfaceType::Unknown {
            String::new()
        } else {
            format!(" · {}", self.interface_type)
        };
        let label = format!(
            "{}{} · {}",
            self.friendly_name,
            interface,
            device_id_suffix(&self.id)
        );
        if self.is_default {
            format!("{label} (default)")
        } else {
            label
        }
    }
}

fn device_id_suffix(id: &str) -> String {
    if let Some((_, suffix)) = id.rsplit_once('{') {
        return suffix.to_string();
    }
    id.chars()
        .rev()
        .take(12)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

/// Enumerate input (microphone) devices. Excludes known virtual devices to
/// prevent feedback loops.
pub fn list_input_devices() -> Result<Vec<AudioDevice>> {
    let host = audio_host()?;
    let default_name = host
        .default_input_device()
        .and_then(|d| device_key(&d))
        .unwrap_or_default();

    Ok(host
        .input_devices()?
        .filter_map(|d| {
            let description = d.description().ok()?;
            if is_virtual_device(&d) {
                return None;
            }
            let id = device_key(&d)?;
            Some(AudioDevice {
                id: id.clone(),
                friendly_name: friendly_name(&description),
                interface_type: description.interface_type(),
                is_default: id == default_name,
            })
        })
        .collect())
}

/// Enumerate output devices.
pub fn list_output_devices() -> Result<Vec<AudioDevice>> {
    let host = audio_host()?;
    let default_name = host
        .default_output_device()
        .and_then(|d| device_key(&d))
        .unwrap_or_default();

    Ok(host
        .output_devices()?
        .filter_map(|d| {
            let description = d.description().ok()?;
            let id = device_key(&d)?;
            Some(AudioDevice {
                id: id.clone(),
                friendly_name: friendly_name(&description),
                interface_type: description.interface_type(),
                is_default: id == default_name,
            })
        })
        .collect())
}

/// UI reads only this snapshot. A single background thread performs WASAPI
/// enumeration; opening a window or clicking the tray never probes drivers.
#[derive(Clone, Default)]
pub struct DeviceSnapshot {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
    pub generation: u64,
    pub error: Option<String>,
}

struct DeviceCatalog {
    snapshot: std::sync::Arc<std::sync::Mutex<DeviceSnapshot>>,
    refresh: std::sync::mpsc::SyncSender<()>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn device_catalog() -> &'static DeviceCatalog {
    static CATALOG: std::sync::OnceLock<DeviceCatalog> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let snapshot = std::sync::Arc::new(std::sync::Mutex::new(DeviceSnapshot::default()));
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (refresh, receiver) = std::sync::mpsc::sync_channel(1);
        let worker_snapshot = std::sync::Arc::clone(&snapshot);
        let worker_busy = std::sync::Arc::clone(&busy);
        if let Err(error) = std::thread::Builder::new()
            .name("device-catalog".into())
            .spawn(move || {
                while receiver.recv().is_ok() {
                    let result = (|| -> Result<_> {
                        let mut inputs = list_input_devices()?;
                        let mut outputs = list_output_devices()?;
                        inputs.truncate(64);
                        outputs.truncate(64);
                        Ok((inputs, outputs))
                    })();
                    let mut cached = worker_snapshot.lock().unwrap_or_else(|e| e.into_inner());
                    cached.generation = cached.generation.wrapping_add(1);
                    match result {
                        Ok((inputs, outputs)) => {
                            cached.inputs = inputs;
                            cached.outputs = outputs;
                            cached.error = None;
                        }
                        Err(error) => {
                            cached.error = Some(error.to_string());
                        }
                    }
                    worker_busy.store(false, std::sync::atomic::Ordering::Release);
                }
            })
        {
            tracing::error!("Cannot start device catalog: {error}");
        }
        DeviceCatalog {
            snapshot,
            refresh,
            busy,
        }
    })
}

pub fn cached_devices() -> DeviceSnapshot {
    device_catalog()
        .snapshot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub fn request_device_refresh() {
    let catalog = device_catalog();
    if catalog.busy.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    if catalog.refresh.try_send(()).is_err() {
        catalog
            .busy
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Auto-detect an installed virtual audio device.
pub fn detect_virtual_device() -> Option<String> {
    let host = audio_host().ok()?;
    let devices: Vec<_> = host.output_devices().ok()?.collect();
    for pattern in VB_CABLE_PATTERNS {
        if let Some(device) = devices
            .iter()
            .find(|device| device_has_name_fragment(device, pattern))
        {
            return device_key(device);
        }
    }
    None
}

pub fn is_vb_cable_device(identifier: &str) -> bool {
    let Ok(host) = audio_host() else {
        return false;
    };
    let Ok(devices) = host.output_devices() else {
        return false;
    };
    devices.into_iter().any(|device| {
        (device_key(&device).is_some_and(|id| id.eq_ignore_ascii_case(identifier))
            || device_name_matches(&device, identifier, true))
            && device_has_vb_cable_name(&device)
    })
}

fn friendly_name(description: &DeviceDescription) -> String {
    description
        .extended()
        .first()
        .cloned()
        .unwrap_or_else(|| description.name().to_string())
}

fn device_key(device: &Device) -> Option<String> {
    device.id().ok().map(|id| id.to_string()).or_else(|| {
        device
            .description()
            .ok()
            .map(|description| description.name().to_string())
    })
}

/// Return the backend endpoint key used for stable device selection and reconnects.
pub fn device_identifier(device: &Device) -> Option<String> {
    device_key(device)
}

fn device_name_matches(device: &Device, value: &str, exact: bool) -> bool {
    let Ok(description) = device.description() else {
        return false;
    };
    let value = value.to_lowercase();
    std::iter::once(description.name())
        .chain(description.extended().iter().map(String::as_str))
        .any(|name| {
            let name = name.to_lowercase();
            if exact {
                name == value
            } else {
                value.contains(&name) || name.contains(&value)
            }
        })
}

fn device_has_name_fragment(device: &Device, value: &str) -> bool {
    let Ok(description) = device.description() else {
        return false;
    };
    let value = value.to_lowercase();
    std::iter::once(description.name())
        .chain(description.extended().iter().map(String::as_str))
        .any(|name| name.to_lowercase().contains(&value))
}

fn is_virtual_device(device: &Device) -> bool {
    VIRTUAL_DEVICE_PATTERNS
        .iter()
        .any(|pattern| device_has_name_fragment(device, pattern))
}

fn device_has_vb_cable_name(device: &Device) -> bool {
    VB_CABLE_PATTERNS
        .iter()
        .any(|pattern| device_has_name_fragment(device, pattern))
}

fn device_label(device: &Device) -> String {
    device
        .description()
        .map(|description| friendly_name(&description))
        .unwrap_or_else(|_| "unknown device".to_string())
}

/// Find an input by stable ID/name, or use the Windows default physical input.
pub fn find_input(name: Option<&str>) -> Result<Device> {
    let host = audio_host()?;
    find_device(&host, name, true)
}

/// Find an output by stable ID or unambiguous name, or return the system default.
pub fn find_output(name: Option<&str>) -> Result<Device> {
    let host = audio_host()?;
    find_device(&host, name, false)
}

fn find_device(host: &Host, name: Option<&str>, is_input: bool) -> Result<Device> {
    match name {
        None => if is_input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }
        .filter(|device| !is_input || !is_virtual_device(device))
        .ok_or_else(|| {
            anyhow!(
                "No eligible default {} device; select a physical microphone explicitly",
                if is_input { "input" } else { "output" }
            )
        }),
        Some(id) => {
            let devices: Vec<_> = if is_input {
                host.input_devices()?.collect()
            } else {
                host.output_devices()?.collect()
            };
            let kind = if is_input { "Input" } else { "Output" };

            let eligible = |device: &&Device| !is_input || !is_virtual_device(device);
            let by_id: Vec<_> = devices
                .iter()
                .filter(eligible)
                .filter(|device| {
                    device_key(device).is_some_and(|device_id| device_id.eq_ignore_ascii_case(id))
                })
                .collect();
            if by_id.len() == 1 {
                return Ok(by_id[0].clone().clone());
            }
            if by_id.len() > 1 {
                return Err(anyhow!("{kind} device ID '{id}' is ambiguous"));
            }

            // Permit a partial device name only when it has one unambiguous
            // match; never route to an arbitrary similarly named microphone.
            let exact_names: Vec<_> = devices
                .iter()
                .filter(eligible)
                .filter(|device| device_name_matches(device, id, true))
                .collect();
            if exact_names.len() == 1 {
                return Ok(exact_names[0].clone().clone());
            }
            if exact_names.len() > 1 {
                return Err(anyhow!(
                    "{kind} device name '{id}' is duplicated; select its full display name or stable ID"
                ));
            }

            let partial_names: Vec<_> = devices
                .iter()
                .filter(eligible)
                .filter(|device| device_name_matches(device, id, false))
                .collect();
            if partial_names.len() == 1 {
                let matched_name = device_label(partial_names[0]);
                tracing::warn!("{kind} device '{id}' not found exactly, matched '{matched_name}'");
                return Ok(partial_names[0].clone().clone());
            }
            if partial_names.len() > 1 {
                return Err(anyhow!("{kind} device name '{id}' is ambiguous"));
            }

            Err(anyhow!("{kind} device '{id}' not found"))
        }
    }
}

/// Pick a supported audio config, preferring F32 then I16, targeting 48 kHz
/// and low channel count. The pipeline has explicit converters for these two
/// formats and rejects any other format instead of building a mismatched stream.
pub fn best_supported_config(device: &Device, is_input: bool) -> Result<SupportedStreamConfig> {
    let configs: Vec<_> = if is_input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };

    // Score a config: prefer F32 > I16 > others, then 48kHz, then low channel count.
    let score = |c: &cpal::SupportedStreamConfigRange| -> i64 {
        let fmt_score: i64 = match c.sample_format() {
            SampleFormat::F32 => 0,
            SampleFormat::I16 => 1_000_000,
            _ => 2_000_000,
        };
        let rate_diff = (c.max_sample_rate() as i64 - 48_000).abs();
        let ch_score = if c.channels() <= 2 {
            0
        } else {
            c.channels() as i64 * 1000
        };
        fmt_score + rate_diff + ch_score
    };

    let best = configs
        .iter()
        .filter(|c| matches!(c.sample_format(), SampleFormat::F32 | SampleFormat::I16))
        .min_by_key(|c| score(c))
        .map(|c| {
            let rate = c.max_sample_rate().min(48_000).max(c.min_sample_rate());
            (*c).with_sample_rate(rate)
        });

    if let Some(cfg) = best {
        return Ok(cfg);
    }

    Err(anyhow!(
        "Device '{}' exposes no supported F32 or I16 {} configuration",
        device_label(device),
        if is_input { "input" } else { "output" }
    ))
}

/// Request approximately 10 ms WASAPI buffers when the device reports that
/// buffer sizing is supported. Other backends retain their default behavior.
pub fn preferred_buffer_size(config: &SupportedStreamConfig) -> BufferSize {
    #[cfg(target_os = "windows")]
    {
        match config.buffer_size() {
            SupportedBufferSize::Range { min, max } if min <= max => {
                let target = (config.sample_rate() / 100).max(1);
                BufferSize::Fixed(target.clamp(*min, *max))
            }
            _ => BufferSize::Default,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = config;
        BufferSize::Default
    }
}

#[cfg(test)]
mod tests {
    use super::preferred_buffer_size;
    use cpal::{BufferSize, SampleFormat, SupportedBufferSize, SupportedStreamConfig};

    #[test]
    fn windows_buffer_request_targets_ten_milliseconds_within_supported_range() {
        let supported = SupportedStreamConfig::new(
            2,
            48_000,
            SupportedBufferSize::Range { min: 128, max: 960 },
            SampleFormat::F32,
        );
        #[cfg(target_os = "windows")]
        assert_eq!(preferred_buffer_size(&supported), BufferSize::Fixed(480));
        #[cfg(not(target_os = "windows"))]
        assert_eq!(preferred_buffer_size(&supported), BufferSize::Default);
    }

    #[test]
    fn windows_buffer_request_clamps_to_the_reported_range() {
        let supported = SupportedStreamConfig::new(
            2,
            48_000,
            SupportedBufferSize::Range {
                min: 512,
                max: 1024,
            },
            SampleFormat::F32,
        );
        #[cfg(target_os = "windows")]
        assert_eq!(preferred_buffer_size(&supported), BufferSize::Fixed(512));
        #[cfg(not(target_os = "windows"))]
        assert_eq!(preferred_buffer_size(&supported), BufferSize::Default);
    }
}
