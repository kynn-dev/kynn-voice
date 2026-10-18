//! Per-process limits. They do not change Windows, drivers, or other processes.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

pub const CPU_LIMIT_PERCENT: u32 = 15;
pub const MEMORY_LIMIT_BYTES: usize = 512 * 1024 * 1024;
const MEMORY_FALLBACK_BYTES: u64 = 384 * 1024 * 1024;
const MEMORY_STOP_BYTES: u64 = 448 * 1024 * 1024;

pub static CPU_PERCENT_X100: AtomicU64 = AtomicU64::new(0);
pub static PRIVATE_BYTES: AtomicU64 = AtomicU64::new(0);
pub static WORKING_SET_BYTES: AtomicU64 = AtomicU64::new(0);
pub static SAFE_MODE: AtomicBool = AtomicBool::new(false);
pub static STOP_AUDIO: AtomicBool = AtomicBool::new(false);

fn pressure_action(private_bytes: u64) -> u8 {
    if private_bytes >= MEMORY_STOP_BYTES {
        2
    } else if private_bytes >= MEMORY_FALLBACK_BYTES {
        1
    } else {
        0
    }
}

pub fn can_load_neural_model() -> bool {
    !SAFE_MODE.load(Ordering::Acquire) && !STOP_AUDIO.load(Ordering::Acquire)
}

#[cfg(target_os = "windows")]
mod windows_limits {
    use super::*;
    use anyhow::Result;
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Instant;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::JobObjects::*;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    use windows::core::PCWSTR;

    pub struct ResourceGuard {
        job: HANDLE,
        stop: Arc<AtomicBool>,
        monitor: Option<JoinHandle<()>>,
    }

    impl ResourceGuard {
        fn create_job() -> Result<Self> {
            let job = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
            let guard = Self {
                job,
                stop: Arc::new(AtomicBool::new(false)),
                monitor: None,
            };
            let memory = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_PROCESS_MEMORY,
                    ..Default::default()
                },
                ProcessMemoryLimit: MEMORY_LIMIT_BYTES,
                ..Default::default()
            };
            let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
                ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE
                    | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
                Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
                    CpuRate: CPU_LIMIT_PERCENT * 100,
                },
            };
            unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &memory as *const _ as _,
                    std::mem::size_of_val(&memory) as u32,
                )?;
                SetInformationJobObject(
                    job,
                    JobObjectCpuRateControlInformation,
                    &cpu as *const _ as _,
                    std::mem::size_of_val(&cpu) as u32,
                )?;
            }
            guard.verify_limits()?;
            Ok(guard)
        }

        /// Fail startup if the requested hard limits cannot be enforced.
        pub fn install() -> Result<Self> {
            let mut guard = Self::create_job()?;
            unsafe {
                AssignProcessToJobObject(guard.job, GetCurrentProcess())?;
            }
            let stop = Arc::clone(&guard.stop);
            guard.monitor = Some(std::thread::Builder::new().name("resource-monitor".into()).spawn(move || {
                let mut previous = cpu_ticks().unwrap_or(0);
                let mut previous_at = Instant::now();
                let cores = std::thread::available_parallelism().map_or(1, usize::from) as f64;
                let mut reports = 0;
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(250));
                    if let Ok((private, working_set)) = memory_usage() {
                        PRIVATE_BYTES.store(private, Ordering::Relaxed);
                        WORKING_SET_BYTES.store(working_set, Ordering::Relaxed);
                        match pressure_action(private) {
                            2 => {
                                SAFE_MODE.store(true, Ordering::Release);
                                if !STOP_AUDIO.swap(true, Ordering::AcqRel) {
                                    tracing::error!("Memory safety stop: private_bytes={private}; restart Kynn Voice to resume audio");
                                }
                            }
                            1 => {
                                if !SAFE_MODE.swap(true, Ordering::AcqRel) {
                                    tracing::warn!("Memory pressure: private_bytes={private}; releasing neural sessions and using RNNoise until restart");
                                }
                            }
                            _ => {}
                        }
                    }
                    let now = Instant::now();
                    if let Ok(ticks) = cpu_ticks() {
                        let elapsed = now.duration_since(previous_at).as_secs_f64();
                        let percent = ticks.saturating_sub(previous) as f64 / 10_000_000.0 / elapsed / cores * 100.0;
                        CPU_PERCENT_X100.store((percent * 100.0).round() as u64, Ordering::Relaxed);
                        previous = ticks;
                        previous_at = now;
                    }
                    reports += 1;
                    if reports % 20 == 0 {
                        tracing::info!("Resources: cpu={:.2}% hard_cap={CPU_LIMIT_PERCENT}% private_mib={:.1} working_set_mib={:.1} memory_cap_mib=512 safe_mode={} stop_audio={}",
                            CPU_PERCENT_X100.load(Ordering::Relaxed) as f64 / 100.0,
                            PRIVATE_BYTES.load(Ordering::Relaxed) as f64 / 1048576.0,
                            WORKING_SET_BYTES.load(Ordering::Relaxed) as f64 / 1048576.0,
                            SAFE_MODE.load(Ordering::Relaxed), STOP_AUDIO.load(Ordering::Relaxed));
                        let audio = &crate::audio::pipeline::AUDIO_METRICS;
                        tracing::info!("Audio metrics: mode={} frames={} dsp_last_us={} dsp_average_us={} dsp_max_us={} latency_last_us={} latency_average_us={} latency_max_us={} callback_max_frames={} rate_adjust_ppm={} underrun_frames={} input_drops={} output_drops={}",
                            audio.active_mode.load(Ordering::Relaxed), audio.processed_frames.load(Ordering::Relaxed),
                            audio.frame_last_us.load(Ordering::Relaxed), audio.frame_average_us.load(Ordering::Relaxed), audio.frame_max_us.load(Ordering::Relaxed),
                            audio.latency_last_us.load(Ordering::Relaxed), audio.latency_average_us.load(Ordering::Relaxed), audio.latency_max_us.load(Ordering::Relaxed),
                            audio.output_callback_frames_max.load(Ordering::Relaxed), audio.output_rate_adjust_ppm.load(Ordering::Relaxed), audio.output_underrun_frames.load(Ordering::Relaxed),
                            audio.input_overrun_frames.load(Ordering::Relaxed), audio.output_overrun_samples.load(Ordering::Relaxed));
                    }
                }
            })?);
            tracing::info!(
                "Windows job limits active: CPU {CPU_LIMIT_PERCENT}% of system capacity; private committed memory 512 MiB"
            );
            Ok(guard)
        }

        pub fn verify_limits(&self) -> Result<()> {
            let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
            let mut memory = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            unsafe {
                QueryInformationJobObject(
                    Some(self.job),
                    JobObjectCpuRateControlInformation,
                    &mut cpu as *mut _ as _,
                    std::mem::size_of_val(&cpu) as u32,
                    None,
                )?;
                QueryInformationJobObject(
                    Some(self.job),
                    JobObjectExtendedLimitInformation,
                    &mut memory as *mut _ as _,
                    std::mem::size_of_val(&memory) as u32,
                    None,
                )?;
                anyhow::ensure!(
                    cpu.Anonymous.CpuRate == CPU_LIMIT_PERCENT * 100,
                    "CPU cap was not applied"
                );
            }
            anyhow::ensure!(
                cpu.ControlFlags.contains(
                    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP
                ),
                "Missing CPU hard cap"
            );
            anyhow::ensure!(
                memory
                    .BasicLimitInformation
                    .LimitFlags
                    .contains(JOB_OBJECT_LIMIT_PROCESS_MEMORY)
                    && memory.ProcessMemoryLimit == MEMORY_LIMIT_BYTES,
                "Missing memory limit"
            );
            Ok(())
        }
    }

    impl Drop for ResourceGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(monitor) = self.monitor.take() {
                let _ = monitor.join();
            }
            unsafe {
                let _ = CloseHandle(self.job);
            }
        }
    }

    pub fn memory_usage() -> Result<(u64, u64)> {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters as *mut _ as _,
                counters.cb,
            )?;
        }
        Ok((counters.PrivateUsage as u64, counters.WorkingSetSize as u64))
    }

    pub fn cpu_ticks() -> Result<u64> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )?;
        }
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Ok(ticks(kernel).saturating_add(ticks(user)))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn windows_accepts_and_reports_cpu_and_memory_hard_limits() {
            ResourceGuard::create_job()
                .expect("create limited job")
                .verify_limits()
                .unwrap();
        }
        #[test]
        fn process_resource_measurements_are_available() {
            let (private, working) = memory_usage().unwrap();
            assert!(private > 0 && working > 0);
            assert!(cpu_ticks().is_ok());
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_limits::{ResourceGuard, cpu_ticks, memory_usage};

/// Standalone diagnostic process: verifies the assigned job, confirms a normal
/// allocation works and the over-limit allocation is rejected, then measures
/// CPU time under a two-second load. The queried CPU quota is in cycles, so
/// the time-based observation is reported without comparing unlike units.
/// It opens no audio devices.
#[cfg(target_os = "windows")]
pub fn self_test(guard: &ResourceGuard) -> anyhow::Result<()> {
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };
    guard.verify_limits()?;
    unsafe {
        let small = VirtualAlloc(None, 1024 * 1024, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        anyhow::ensure!(!small.is_null(), "Control allocation failed");
        VirtualFree(small, 0, MEM_RELEASE)?;
        let oversized = VirtualAlloc(
            None,
            MEMORY_LIMIT_BYTES + 64 * 1024 * 1024,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if !oversized.is_null() {
            VirtualFree(oversized, 0, MEM_RELEASE)?;
            anyhow::bail!("Windows did not reject the allocation over the memory cap");
        }
        let error = windows::Win32::Foundation::GetLastError();
        anyhow::ensure!(
            error.0 == 8 || error.0 == 1455,
            "Unexpected allocation error: {}",
            error.0
        );
    }
    let processors = std::thread::available_parallelism().map_or(1, usize::from);
    let started = std::time::Instant::now();
    let ticks_before = cpu_ticks()?;
    let deadline = started + Duration::from_secs(2);
    let mut workers = Vec::with_capacity(processors.min(8));
    for _ in 0..processors.min(8) {
        workers.push(std::thread::spawn(move || {
            while std::time::Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }));
    }
    for worker in workers {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("CPU probe worker panicked"))?;
    }
    let cpu_percent = cpu_ticks()?.saturating_sub(ticks_before) as f64
        / 10_000_000.0
        / started.elapsed().as_secs_f64()
        / processors as f64
        * 100.0;
    let memory = memory_usage()?;
    println!(
        "RESOURCE_SELF_TEST_PASS cpu_cycle_quota_verified=15% memory_hard_cap_mib=512 oversized_allocation=REJECTED cpu_time_observed={cpu_percent:.2}% private_bytes={} working_set_bytes={}",
        memory.0, memory.1
    );
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub struct ResourceGuard;
#[cfg(not(target_os = "windows"))]
impl ResourceGuard {
    pub fn install() -> anyhow::Result<Self> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_pressure_falls_back_before_the_hard_limit() {
        assert_eq!(pressure_action(383 * 1024 * 1024), 0);
        assert_eq!(pressure_action(384 * 1024 * 1024), 1);
        assert_eq!(pressure_action(448 * 1024 * 1024), 2);
        assert!(MEMORY_STOP_BYTES < MEMORY_LIMIT_BYTES as u64);
    }
}
