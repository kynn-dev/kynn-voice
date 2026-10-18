use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use ringbuf::traits::{Consumer, Observer, Producer, Split};

use crate::config::{Config, ProcessingMode, RuntimeSettings};
use crate::dsp::autogain::AutoGain;
use crate::dsp::deepfilter::DeepFilterProcessor;
use crate::dsp::denoise::{Denoiser, FRAME_SIZE};
use crate::dsp::energy_vad::EnergyVad;
use crate::dsp::eq::ThreeBandEq;
use crate::dsp::gate::NoiseGate;
use crate::dsp::highpass::HighPassFilter;
use crate::dsp::{ProcessResult, Processor};

use super::device;
use super::resample::StreamResampler;

const ENGINE_RATE: u32 = 48_000;
const CAPTURE_QUEUE_FRAMES: usize = 8;
const OUTPUT_QUEUE_SAMPLES: usize = FRAME_SIZE * 8;
const MAX_PENDING_SAMPLES: usize = FRAME_SIZE * 16;
const OUTPUT_PREFILL_MS: usize = 20;
const NO_ACTIVE_MODE: u8 = u8::MAX;
// Measured with the pinned RNNoise port; DeepFilterNet's centered 960-sample
// overlap-add emits the prior 480-sample hop.
const RNNOISE_DRY_DELAY_SAMPLES: usize = 472;
const DEEPFILTER_DRY_DELAY_SAMPLES: usize = FRAME_SIZE;
static DEEPFILTER_LOAD_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static DSP_WORKERS_ALIVE: AtomicUsize = AtomicUsize::new(0);

struct WorkerAdmission;
impl WorkerAdmission {
    fn acquire() -> Result<Self> {
        DSP_WORKERS_ALIVE
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| anyhow!("Previous DSP worker is still finishing; restart deferred"))?;
        Ok(Self)
    }
}
impl Drop for WorkerAdmission {
    fn drop(&mut self) {
        DSP_WORKERS_ALIVE.store(0, Ordering::Release);
    }
}

/// Monotonically increasing pipeline ID — used to invalidate stale callbacks.
static PIPELINE_ID: AtomicU64 = AtomicU64::new(0);

/// Current input RMS (f32 bits stored in an atomic for the settings meter).
pub static AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);
/// Current processed-output RMS (f32 bits stored in an atomic for the meter).
pub static OUTPUT_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Lock-free audio diagnostics, readable from the UI thread.
pub struct AudioMetrics {
    pub frame_last_us: AtomicU64,
    pub frame_average_us: AtomicU64,
    pub frame_max_us: AtomicU64,
    /// Average DSP worker use relative to one 10 ms audio-frame deadline,
    /// expressed in tenths of a percent.
    pub dsp_cpu_percent_x10: AtomicU64,
    /// Software latency from input callback timestamp to output callback.
    pub latency_last_us: AtomicU64,
    pub latency_average_us: AtomicU64,
    pub latency_max_us: AtomicU64,
    /// Number of 10 ms source frames dropped from the capture ring.
    pub input_overrun_frames: AtomicU64,
    /// Number of output samples dropped because a destination ring was full.
    pub output_overrun_samples: AtomicU64,
    /// Number of output frames rendered as silence because the destination ring
    /// had no processed sample available.
    pub output_underrun_frames: AtomicU64,
    /// Largest output callback measured in per-channel audio frames.
    pub output_callback_frames_max: AtomicU64,
    /// Last output clock correction in parts per million, bounded to +/- 2000.
    pub output_rate_adjust_ppm: AtomicI32,
    pub processed_frames: AtomicU64,
    pub active_mode: AtomicU8,
}
impl AudioMetrics {
    const fn new() -> Self {
        Self {
            frame_last_us: AtomicU64::new(0),
            frame_average_us: AtomicU64::new(0),
            frame_max_us: AtomicU64::new(0),
            dsp_cpu_percent_x10: AtomicU64::new(0),
            latency_last_us: AtomicU64::new(0),
            latency_average_us: AtomicU64::new(0),
            latency_max_us: AtomicU64::new(0),
            input_overrun_frames: AtomicU64::new(0),
            output_overrun_samples: AtomicU64::new(0),
            output_underrun_frames: AtomicU64::new(0),
            output_callback_frames_max: AtomicU64::new(0),
            output_rate_adjust_ppm: AtomicI32::new(0),
            processed_frames: AtomicU64::new(0),
            active_mode: AtomicU8::new(NO_ACTIVE_MODE),
        }
    }

    fn record_frame_time(&self, elapsed: Duration) {
        let elapsed_us = duration_us(elapsed);
        self.frame_last_us.store(elapsed_us, Ordering::Relaxed);
        let average = update_average(&self.frame_average_us, elapsed_us);
        update_max(&self.frame_max_us, elapsed_us);
        self.dsp_cpu_percent_x10
            .store(average / 10, Ordering::Relaxed);
        self.processed_frames.fetch_add(1, Ordering::Relaxed);
    }

    fn record_latency(&self, elapsed: Duration) {
        let elapsed_us = duration_us(elapsed);
        self.latency_last_us.store(elapsed_us, Ordering::Relaxed);
        update_average(&self.latency_average_us, elapsed_us);
        update_max(&self.latency_max_us, elapsed_us);
    }
}

pub static AUDIO_METRICS: AudioMetrics = AudioMetrics::new();

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn update_average(cell: &AtomicU64, value: u64) -> u64 {
    let mut current = cell.load(Ordering::Relaxed);
    loop {
        let next = if current == 0 {
            value
        } else {
            current.saturating_mul(15).saturating_add(value) / 16
        };
        match cell.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => current = actual,
        }
    }
}

fn update_max(cell: &AtomicU64, value: u64) {
    let mut current = cell.load(Ordering::Relaxed);
    while value > current {
        match cell.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

#[derive(Clone, Copy)]
struct CapturedFrame {
    samples: [f32; FRAME_SIZE],
    captured_at: Instant,
}

#[derive(Clone, Copy)]
struct TimedSample {
    sample: f32,
    captured_at: Instant,
}

/// Converts interleaved device samples into fixed-size mono frames without
/// allocating or waiting inside the CPAL input callback.
struct CaptureWriter {
    producer: ringbuf::HeapProd<CapturedFrame>,
    channels: usize,
    sample_rate: u32,
    position: usize,
    frame: [f32; FRAME_SIZE],
    frame_start: Instant,
}

impl CaptureWriter {
    fn new(producer: ringbuf::HeapProd<CapturedFrame>, channels: usize, sample_rate: u32) -> Self {
        Self {
            producer,
            channels,
            sample_rate,
            position: 0,
            frame: [0.0; FRAME_SIZE],
            frame_start: Instant::now(),
        }
    }

    fn capture_f32(&mut self, data: &[f32], callback_at: Instant, gain: f32) {
        for (frame_index, channels) in data.chunks_exact(self.channels).enumerate() {
            let mono = channels.iter().copied().sum::<f32>() / self.channels as f32;
            self.push_sample(mono * gain, callback_at, frame_index);
        }
    }

    fn capture_i16(&mut self, data: &[i16], callback_at: Instant, gain: f32) {
        const I16_SCALE: f32 = 1.0 / 32768.0;
        for (frame_index, channels) in data.chunks_exact(self.channels).enumerate() {
            let mono = channels
                .iter()
                .map(|&sample| sample as f32 * I16_SCALE)
                .sum::<f32>()
                / self.channels as f32;
            self.push_sample(mono * gain, callback_at, frame_index);
        }
    }

    fn push_sample(&mut self, sample: f32, callback_at: Instant, sample_index: usize) {
        if self.position == 0 {
            let offset_us =
                (sample_index as u64).saturating_mul(1_000_000) / self.sample_rate.max(1) as u64;
            self.frame_start = callback_at + Duration::from_micros(offset_us);
        }

        self.frame[self.position] = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.position += 1;

        if self.position == FRAME_SIZE {
            let packet = CapturedFrame {
                samples: self.frame,
                captured_at: self.frame_start,
            };
            if self.producer.try_push(packet).is_err() {
                AUDIO_METRICS
                    .input_overrun_frames
                    .fetch_add(1, Ordering::Relaxed);
            }
            self.position = 0;
        }
    }
}

struct OutputReader {
    consumer: ringbuf::HeapCons<TimedSample>,
    channels: usize,
    pipeline_id: u64,
    is_monitor: bool,
    settings: Arc<RuntimeSettings>,
    prefill_samples: usize,
    jitter_samples: usize,
    primed: bool,
    current: Option<TimedSample>,
    next: Option<TimedSample>,
    phase: f64,
    rate_ratio: f64,
    clock_integral: f64,
}

impl OutputReader {
    fn new(
        consumer: ringbuf::HeapCons<TimedSample>,
        channels: usize,
        pipeline_id: u64,
        is_monitor: bool,
        settings: Arc<RuntimeSettings>,
        sample_rate: u32,
    ) -> Self {
        Self {
            consumer,
            channels,
            pipeline_id,
            is_monitor,
            settings,
            prefill_samples: (sample_rate as usize * OUTPUT_PREFILL_MS / 1000)
                .min(OUTPUT_QUEUE_SAMPLES),
            jitter_samples: (sample_rate as usize / 100).max(1),
            primed: false,
            current: None,
            next: None,
            phase: 0.0,
            rate_ratio: 1.0,
            clock_integral: 0.0,
        }
    }

    fn render_f32(&mut self, data: &mut [f32]) {
        data.fill(0.0);
        self.record_callback_size(data.len());
        if PIPELINE_ID.load(Ordering::Acquire) != self.pipeline_id
            || self.channels == 0
            || !self.prime_if_ready(data.len() / self.channels)
        {
            return;
        }

        self.update_clock_ratio(data.len() / self.channels);
        let gain = self.gain();
        let mut missing = 0u64;
        let mut oldest_sample = None;
        for frame in data.chunks_exact_mut(self.channels) {
            if let Some(sample) = self.next_output() {
                oldest_sample.get_or_insert(sample.captured_at);
                let output = (sample.sample * gain).clamp(-1.0, 1.0);
                frame.fill(output);
            } else {
                missing += 1;
            }
        }
        self.record_callback(missing, oldest_sample);
    }

    fn render_i16(&mut self, data: &mut [i16]) {
        data.fill(0);
        self.record_callback_size(data.len());
        if PIPELINE_ID.load(Ordering::Acquire) != self.pipeline_id
            || self.channels == 0
            || !self.prime_if_ready(data.len() / self.channels)
        {
            return;
        }

        self.update_clock_ratio(data.len() / self.channels);
        let gain = self.gain();
        let mut missing = 0u64;
        let mut oldest_sample = None;
        for frame in data.chunks_exact_mut(self.channels) {
            if let Some(sample) = self.next_output() {
                oldest_sample.get_or_insert(sample.captured_at);
                frame.fill(to_i16(sample.sample * gain));
            } else {
                missing += 1;
            }
        }
        self.record_callback(missing, oldest_sample);
    }

    fn gain(&self) -> f32 {
        if !self.is_monitor {
            return 1.0;
        }
        let gain = f32::from_bits(self.settings.output_gain.load(Ordering::Relaxed));
        if gain.is_finite() {
            gain.clamp(0.0, 4.0)
        } else {
            1.0
        }
    }

    fn update_clock_ratio(&mut self, callback_frames: usize) {
        // USB capture and a virtual render endpoint have independent clocks.
        // Regulate queue occupancy with a slowly changing, tiny ratio correction
        // rather than periodically dropping/duplicating a whole 10 ms frame.
        let available = self.consumer.occupied_len()
            + usize::from(self.current.is_some())
            + usize::from(self.next.is_some());
        let error = available as f64 - self.prefill_samples as f64;
        let normalized = error / self.jitter_samples as f64;
        let dt = callback_frames as f64 / (self.jitter_samples as f64 * 100.0);
        self.clock_integral = (self.clock_integral + normalized * dt * 0.0002).clamp(-0.002, 0.002);
        let desired = 1.0 + (normalized * 0.004 + self.clock_integral).clamp(-0.002, 0.002);
        self.rate_ratio += (desired - self.rate_ratio) * 0.02;
        if !self.is_monitor {
            AUDIO_METRICS.output_rate_adjust_ppm.store(
                ((self.rate_ratio - 1.0) * 1_000_000.0).round() as i32,
                Ordering::Relaxed,
            );
        }
    }

    fn next_output(&mut self) -> Option<TimedSample> {
        if self.current.is_none() {
            self.current = self.consumer.try_pop();
        }
        if self.next.is_none() {
            self.next = self.consumer.try_pop();
        }
        let current = self.current.as_ref()?;
        let next = self.next.as_ref()?;
        let sample = TimedSample {
            sample: current.sample + (next.sample - current.sample) * self.phase as f32,
            captured_at: current.captured_at,
        };
        self.phase += self.rate_ratio;
        // rate_ratio is in [0.998, 1.002]; at most two advances per output
        // sample, with no allocation, locks, driver calls, or DSP inference.
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.current = self.next.take();
            self.next = self.consumer.try_pop();
        }
        Some(sample)
    }

    fn prime_if_ready(&mut self, callback_frames: usize) -> bool {
        // WASAPI can deliver a larger callback than the requested Fixed(480).
        // Start/recover with an entire observed callback plus 10 ms of margin.
        // The ring remains bounded; no buffer allocation occurs here.
        self.prefill_samples = self.prefill_samples.max(
            callback_frames
                .saturating_add(self.jitter_samples)
                .min(OUTPUT_QUEUE_SAMPLES),
        );
        if self.primed {
            return true;
        }
        if self.consumer.occupied_len() < self.prefill_samples {
            return false;
        }
        self.primed = true;
        true
    }

    fn record_callback_size(&self, samples: usize) {
        if self.channels > 0 {
            update_max(
                &AUDIO_METRICS.output_callback_frames_max,
                (samples / self.channels) as u64,
            );
        }
    }

    fn record_callback(&mut self, missing_frames: u64, oldest_sample: Option<Instant>) {
        if missing_frames > 0 {
            self.primed = false;
            self.current = None;
            self.next = None;
            self.phase = 0.0;
            AUDIO_METRICS
                .output_underrun_frames
                .fetch_add(missing_frames, Ordering::Relaxed);
        }
        if let Some(captured_at) = oldest_sample {
            AUDIO_METRICS.record_latency(captured_at.elapsed());
        }
    }
}

fn to_i16(sample: f32) -> i16 {
    let sample = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if sample <= -1.0 {
        i16::MIN
    } else if sample >= 1.0 {
        i16::MAX
    } else if sample < 0.0 {
        (sample * 32768.0) as i16
    } else {
        (sample * 32767.0) as i16
    }
}

struct OutputInfo {
    stream: Stream,
    producer: ringbuf::HeapProd<TimedSample>,
    rate: u32,
}

struct OutputProducer {
    producer: ringbuf::HeapProd<TimedSample>,
    resampler: Option<StreamResampler>,
}

fn build_output(
    device_name: &str,
    label: &str,
    pipeline_id: u64,
    settings: &Arc<RuntimeSettings>,
    error_flag: &Arc<AtomicBool>,
) -> Result<OutputInfo> {
    let output_device = device::find_output(Some(device_name))?;
    let supported = device::best_supported_config(&output_device, false)?;
    let sample_format = supported.sample_format();
    let buffer_size = device::preferred_buffer_size(&supported);
    let mut config: StreamConfig = supported.into();
    config.buffer_size = buffer_size;
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    if channels == 0 {
        return Err(anyhow!(
            "{label} device '{device_name}' reports zero channels"
        ));
    }

    let friendly_name = output_device
        .description()
        .map(|description| {
            description
                .extended()
                .first()
                .cloned()
                .unwrap_or_else(|| description.name().to_string())
        })
        .unwrap_or_else(|_| "unknown device".to_string());
    tracing::info!(
        "{label}: {friendly_name} [{device_name}] | {channels} ch | {rate} Hz | {sample_format:?} | buffer {buffer_size:?}"
    );

    let ring = ringbuf::HeapRb::<TimedSample>::new(OUTPUT_QUEUE_SAMPLES);
    let (producer, consumer) = ring.split();
    let stream = match sample_format {
        SampleFormat::F32 => {
            let mut reader = OutputReader::new(
                consumer,
                channels,
                pipeline_id,
                label == "Monitor",
                Arc::clone(settings),
                rate,
            );
            let callback_error = Arc::clone(error_flag);
            output_device.build_output_stream(
                &config,
                move |data: &mut [f32], _| reader.render_f32(data),
                move |error| {
                    let _ = error;
                    if PIPELINE_ID.load(Ordering::Acquire) == pipeline_id {
                        callback_error.store(true, Ordering::Release);
                    }
                },
                None,
            )?
        }
        SampleFormat::I16 => {
            let mut reader = OutputReader::new(
                consumer,
                channels,
                pipeline_id,
                label == "Monitor",
                Arc::clone(settings),
                rate,
            );
            let callback_error = Arc::clone(error_flag);
            output_device.build_output_stream(
                &config,
                move |data: &mut [i16], _| reader.render_i16(data),
                move |error| {
                    let _ = error;
                    if PIPELINE_ID.load(Ordering::Acquire) == pipeline_id {
                        callback_error.store(true, Ordering::Release);
                    }
                },
                None,
            )?
        }
        other => {
            return Err(anyhow!(
                "Unsupported {label} output sample format {other:?}; F32 or I16 is required"
            ));
        }
    };

    Ok(OutputInfo {
        stream,
        producer,
        rate,
    })
}

fn make_output_producer(
    producer: ringbuf::HeapProd<TimedSample>,
    rate: u32,
) -> Result<OutputProducer> {
    let resampler = if (rate as i64 - ENGINE_RATE as i64).unsigned_abs() >= 1 {
        Some(StreamResampler::new(
            ENGINE_RATE as usize,
            rate as usize,
            FRAME_SIZE,
        )?)
    } else {
        None
    };
    Ok(OutputProducer {
        producer,
        resampler,
    })
}

/// Owns all CPAL streams and the worker that performs resampling and DSP.
struct PipelineInner {
    _input_stream: Stream,
    _output_streams: Vec<Stream>,
    worker_stop: Arc<AtomicBool>,
    worker_finished: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
}

// SAFETY: The streams stay in PipelineInner and are only accessed while the
// Pipeline control mutex is held. Audio data callbacks never access this value.
unsafe impl Send for PipelineInner {}

#[derive(Clone, Default)]
struct DeviceConfig {
    input: Option<String>,
    monitor: Option<String>,
    virtual_dev: Option<String>,
}

pub struct Pipeline {
    control: Mutex<()>,
    inner: Mutex<Option<PipelineInner>>,
    pub settings: Arc<RuntimeSettings>,
    error_flag: Arc<AtomicBool>,
    device_config: Mutex<DeviceConfig>,
    running: AtomicBool,
    desired_running: AtomicBool,
    pending_control: Mutex<Option<bool>>,
    control_wake: mpsc::SyncSender<()>,
    control_receiver: Mutex<Option<mpsc::Receiver<()>>>,
    last_error: Mutex<Option<String>>,
    active_input_key: Mutex<Option<String>>,
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("Mutex was poisoned — recovering");
        poisoned.into_inner()
    })
}

impl Pipeline {
    pub fn new(settings: Arc<RuntimeSettings>) -> Self {
        let (control_wake, control_receiver) = mpsc::sync_channel(1);
        Self {
            control: Mutex::new(()),
            inner: Mutex::new(None),
            settings,
            error_flag: Arc::new(AtomicBool::new(false)),
            device_config: Mutex::new(DeviceConfig::default()),
            running: AtomicBool::new(false),
            desired_running: AtomicBool::new(false),
            pending_control: Mutex::new(None),
            control_wake,
            control_receiver: Mutex::new(Some(control_receiver)),
            last_error: Mutex::new(None),
            active_input_key: Mutex::new(None),
        }
    }

    pub fn start(
        &self,
        input_name: Option<&str>,
        monitor_name: Option<&str>,
        virtual_name: Option<&str>,
    ) -> Result<()> {
        let _control = lock_or_recover(&self.control);
        self.desired_running.store(true, Ordering::Release);
        self.remember_devices(input_name, monitor_name, virtual_name);
        self.stop_inner();
        self.error_flag.store(false, Ordering::Release);

        match self.build_and_start(input_name, monitor_name, virtual_name) {
            Ok(()) => {
                self.running.store(true, Ordering::Release);
                *lock_or_recover(&self.last_error) = None;
                Ok(())
            }
            Err(error) => {
                self.stop_inner();
                *lock_or_recover(&self.last_error) = Some(error.to_string());
                self.error_flag.store(true, Ordering::Release);
                Err(error)
            }
        }
    }

    fn build_and_start(
        &self,
        input_name: Option<&str>,
        monitor_name: Option<&str>,
        virtual_name: Option<&str>,
    ) -> Result<()> {
        anyhow::ensure!(
            !crate::resources::STOP_AUDIO.load(Ordering::Acquire),
            "Audio stopped by memory protection; restart Kynn Voice"
        );
        // The guard moves into the worker closure. Even after a timed-out
        // shutdown it prevents a second live DSP worker from being created.
        let admission = WorkerAdmission::acquire()?;
        let pipeline_id = PIPELINE_ID.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        let input_device = device::find_input(input_name)?;
        *lock_or_recover(&self.active_input_key) = device::device_identifier(&input_device);
        let input_supported = device::best_supported_config(&input_device, true)?;
        let input_sample_format = input_supported.sample_format();
        let input_buffer_size = device::preferred_buffer_size(&input_supported);
        let mut input_config: StreamConfig = input_supported.into();
        input_config.buffer_size = input_buffer_size;
        let input_channels = input_config.channels as usize;
        let input_rate = input_config.sample_rate;
        if input_channels == 0 {
            return Err(anyhow!("Input device reports zero channels"));
        }
        if !matches!(input_sample_format, SampleFormat::F32 | SampleFormat::I16) {
            return Err(anyhow!(
                "Unsupported input sample format {input_sample_format:?}; F32 or I16 is required"
            ));
        }

        let input_description = input_device
            .description()
            .map(|description| {
                description
                    .extended()
                    .first()
                    .cloned()
                    .unwrap_or_else(|| description.name().to_string())
            })
            .unwrap_or_default();
        let input_id = input_device
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown endpoint ID".to_string());
        tracing::info!(
            "Input: {input_description} [{input_id}] | {input_channels} ch | {input_rate} Hz | {input_sample_format:?} | buffer {input_buffer_size:?}"
        );

        let settings = Arc::clone(&self.settings);
        let error_flag = Arc::clone(&self.error_flag);
        let mut output_streams = Vec::with_capacity(2);
        let mut output_producers = Vec::with_capacity(2);

        let virtual_device = virtual_name
            .map(str::to_string)
            .or_else(device::detect_virtual_device)
            .ok_or_else(|| {
                anyhow!(
                    "VB-CABLE output was not found. Install VB-CABLE manually, then select its 'CABLE Input' endpoint."
                )
            })?;
        if !device::is_vb_cable_device(&virtual_device) {
            return Err(anyhow!(
                "Virtual output '{virtual_device}' is not a recognized VB-CABLE endpoint"
            ));
        }

        if let Some(monitor_name) = monitor_name.filter(|name| *name != virtual_device.as_str()) {
            let monitor =
                build_output(monitor_name, "Monitor", pipeline_id, &settings, &error_flag)?;
            output_streams.push(monitor.stream);
            output_producers.push(make_output_producer(monitor.producer, monitor.rate)?);
        }

        let virtual_output = build_output(
            &virtual_device,
            "VB-CABLE",
            pipeline_id,
            &settings,
            &error_flag,
        )?;
        output_streams.push(virtual_output.stream);
        output_producers.push(make_output_producer(
            virtual_output.producer,
            virtual_output.rate,
        )?);

        let capture_ring = ringbuf::HeapRb::<CapturedFrame>::new(CAPTURE_QUEUE_FRAMES);
        let (capture_producer, capture_consumer) = capture_ring.split();
        let input_stream = build_input(
            input_device,
            &input_config,
            input_sample_format,
            input_channels,
            input_rate,
            capture_producer,
            pipeline_id,
            &settings,
            &error_flag,
        )?;

        let worker_stop = Arc::new(AtomicBool::new(false));
        let worker_stop_clone = Arc::clone(&worker_stop);
        let worker_settings = Arc::clone(&settings);
        let worker_error = Arc::clone(&error_flag);
        let (finished_tx, finished_rx) = mpsc::channel();
        AUDIO_METRICS
            .active_mode
            .store(NO_ACTIVE_MODE, Ordering::Relaxed);
        let worker = thread::Builder::new()
            .name("audio-dsp".into())
            .spawn(move || {
                let _admission = admission;
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run_worker(
                        pipeline_id,
                        capture_consumer,
                        output_producers,
                        input_rate,
                        worker_settings,
                        worker_stop_clone,
                    )
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::error!("Audio DSP worker stopped: {error}");
                        if PIPELINE_ID.load(Ordering::Acquire) == pipeline_id {
                            worker_error.store(true, Ordering::Release);
                        }
                    }
                    Err(_) => {
                        tracing::error!("Audio DSP worker panicked; watchdog will restart it");
                        if PIPELINE_ID.load(Ordering::Acquire) == pipeline_id {
                            worker_error.store(true, Ordering::Release);
                        }
                    }
                }
                let _ = finished_tx.send(());
            })?;

        *lock_or_recover(&self.inner) = Some(PipelineInner {
            _input_stream: input_stream,
            _output_streams: output_streams,
            worker_stop,
            worker_finished: finished_rx,
            worker: Some(worker),
        });

        let inner = lock_or_recover(&self.inner);
        let Some(inner) = inner.as_ref() else {
            return Err(anyhow!("Pipeline streams disappeared during startup"));
        };
        for stream in &inner._output_streams {
            stream.play()?;
        }
        inner._input_stream.play()?;

        tracing::info!("Pipeline started on WASAPI at a 48 kHz internal rate");
        Ok(())
    }

    fn stop_inner(&self) {
        self.running.store(false, Ordering::Release);
        PIPELINE_ID.fetch_add(1, Ordering::AcqRel);
        AUDIO_METRICS
            .active_mode
            .store(NO_ACTIVE_MODE, Ordering::Relaxed);
        let previous = lock_or_recover(&self.inner).take();
        if let Some(mut previous) = previous {
            previous.worker_stop.store(true, Ordering::Release);
            if let Some(worker) = previous.worker.take() {
                if previous
                    .worker_finished
                    .recv_timeout(Duration::from_millis(100))
                    .is_ok()
                {
                    let _ = worker.join();
                } else {
                    // A long inference or worker shutdown may still be finishing.
                    // Detach after a bounded wait so Stop/Quit cannot hang.
                    tracing::warn!("DSP worker is still shutting down; detaching it");
                    drop(worker);
                }
            }
            drop(previous);
            AUDIO_LEVEL.store(0.0f32.to_bits(), Ordering::Relaxed);
            OUTPUT_LEVEL.store(0.0f32.to_bits(), Ordering::Relaxed);
            tracing::info!("Pipeline stopped");
        }
    }

    pub fn stop(&self) {
        let _control = lock_or_recover(&self.control);
        self.stop_inner();
        self.error_flag.store(false, Ordering::Release);
        self.desired_running.store(false, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn wants_to_run(&self) -> bool {
        self.desired_running.load(Ordering::Acquire)
    }

    pub fn last_error(&self) -> Option<String> {
        lock_or_recover(&self.last_error).clone()
    }

    /// UI requests coalesce into one slot. Device I/O and stream destruction
    /// always execute on the controller rather than the window thread.
    pub fn request_running(&self, running: bool) {
        self.desired_running.store(running, Ordering::Release);
        *lock_or_recover(&self.pending_control) = Some(running);
        let _ = self.control_wake.try_send(());
    }

    pub fn request_devices(
        &self,
        input: Option<&str>,
        monitor: Option<&str>,
        virtual_dev: Option<&str>,
    ) {
        self.remember_devices(input, monitor, virtual_dev);
        if self.desired_running.load(Ordering::Acquire) {
            self.request_running(true);
        }
    }

    pub fn has_error(&self) -> bool {
        self.error_flag.load(Ordering::Acquire)
    }

    /// Snapshot current device selections and live DSP settings for saving.
    pub fn to_config(&self, base: &Config) -> Config {
        let devices = lock_or_recover(&self.device_config).clone();
        let mut snapshot = base.clone();
        snapshot.input_device = devices.input;
        snapshot.output_device = devices.monitor;
        snapshot.virtual_device = devices.virtual_dev;
        snapshot.start_processing = self.wants_to_run();
        self.settings.to_config(&snapshot)
    }

    pub fn remember_devices(
        &self,
        input_name: Option<&str>,
        monitor_name: Option<&str>,
        virtual_name: Option<&str>,
    ) {
        *lock_or_recover(&self.device_config) = DeviceConfig {
            input: input_name.map(String::from),
            monitor: monitor_name.map(String::from),
            virtual_dev: virtual_name.map(String::from),
        };
    }

    fn configured_input_is_missing(&self) -> bool {
        let config = lock_or_recover(&self.device_config).clone();
        match device::find_input(config.input.as_deref()) {
            Ok(input) => {
                config.input.is_none()
                    && *lock_or_recover(&self.active_input_key) != device::device_identifier(&input)
            }
            Err(_) => true,
        }
    }

    pub fn try_reconnect(&self) -> Result<()> {
        let config = lock_or_recover(&self.device_config).clone();
        self.start(
            config.input.as_deref(),
            config.monitor.as_deref(),
            config.virtual_dev.as_deref(),
        )
    }

    pub fn start_watchdog(pipeline: Arc<Pipeline>) -> WatchdogHandle {
        let receiver = lock_or_recover(&pipeline.control_receiver)
            .take()
            .expect("Only one audio controller is allowed");
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let handle = thread::Builder::new()
            .name("audio-watchdog".into())
            .spawn(move || {
                let mut last_check = Instant::now();
                while running_clone.load(Ordering::Relaxed) {
                    let _ = receiver.recv_timeout(Duration::from_millis(250));
                    if !running_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    let request = lock_or_recover(&pipeline.pending_control).take();
                    if let Some(run) = request {
                        if run {
                            if let Err(error) = pipeline.try_reconnect() {
                                tracing::warn!("Requested audio start failed: {error}");
                            }
                        } else {
                            pipeline.stop();
                        }
                        last_check = Instant::now();
                    }
                    if crate::resources::STOP_AUDIO.load(Ordering::Acquire) {
                        if pipeline.is_running() {
                            pipeline.stop();
                        }
                        continue;
                    }
                    if !pipeline.desired_running.load(Ordering::Acquire)
                        || last_check.elapsed() < Duration::from_secs(2)
                    {
                        continue;
                    }
                    last_check = Instant::now();

                    if pipeline.is_running() && pipeline.configured_input_is_missing() {
                        pipeline.error_flag.store(true, Ordering::Release);
                    }
                    if pipeline.has_error() {
                        tracing::warn!("Audio device error detected — retrying saved devices");
                        match pipeline.try_reconnect() {
                            Ok(()) => tracing::info!("Audio device reconnection succeeded"),
                            Err(error) => {
                                tracing::warn!("Audio reconnect attempt failed: {error}");
                            }
                        }
                    }
                }
            })
            .expect("Failed to spawn audio watchdog thread");

        WatchdogHandle {
            running,
            _handle: Some(handle),
        }
    }
}

fn build_input(
    input_device: cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    channels: usize,
    rate: u32,
    producer: ringbuf::HeapProd<CapturedFrame>,
    pipeline_id: u64,
    settings: &Arc<RuntimeSettings>,
    error_flag: &Arc<AtomicBool>,
) -> Result<Stream> {
    match sample_format {
        SampleFormat::F32 => {
            let mut capture = CaptureWriter::new(producer, channels, rate);
            let settings = Arc::clone(settings);
            let callback_error = Arc::clone(error_flag);
            Ok(input_device.build_input_stream(
                config,
                move |data: &[f32], _| {
                    if PIPELINE_ID.load(Ordering::Acquire) != pipeline_id {
                        return;
                    }
                    let gain = input_gain(&settings);
                    capture.capture_f32(data, Instant::now(), gain);
                },
                move |error| {
                    let _ = error;
                    if PIPELINE_ID.load(Ordering::Acquire) == pipeline_id {
                        callback_error.store(true, Ordering::Release);
                    }
                },
                None,
            )?)
        }
        SampleFormat::I16 => {
            let mut capture = CaptureWriter::new(producer, channels, rate);
            let settings = Arc::clone(settings);
            let callback_error = Arc::clone(error_flag);
            Ok(input_device.build_input_stream(
                config,
                move |data: &[i16], _| {
                    if PIPELINE_ID.load(Ordering::Acquire) != pipeline_id {
                        return;
                    }
                    let gain = input_gain(&settings);
                    capture.capture_i16(data, Instant::now(), gain);
                },
                move |error| {
                    let _ = error;
                    if PIPELINE_ID.load(Ordering::Acquire) == pipeline_id {
                        callback_error.store(true, Ordering::Release);
                    }
                },
                None,
            )?)
        }
        other => Err(anyhow!("Unsupported input sample format {other:?}")),
    }
}

fn input_gain(settings: &RuntimeSettings) -> f32 {
    let gain = f32::from_bits(settings.input_gain.load(Ordering::Relaxed));
    if gain.is_finite() {
        gain.clamp(0.0, 4.0)
    } else {
        1.0
    }
}

enum ActiveDenoiser {
    Light(Denoiser),
    DeepFilter(DeepFilterProcessor),
}

impl ActiveDenoiser {
    fn load(mode: ProcessingMode) -> Result<Self> {
        match mode {
            ProcessingMode::Light => Ok(Self::Light(Denoiser::new())),
            ProcessingMode::Balanced | ProcessingMode::Studio => {
                crate::models::init_ort()?;
                let model_dir = crate::models::ensure_deepfilter_model()?;
                Ok(Self::DeepFilter(DeepFilterProcessor::from_model_dir(
                    &model_dir,
                )?))
            }
        }
    }

    fn process(&mut self, frame: &mut [f32]) -> ProcessResult {
        match self {
            Self::Light(denoiser) => denoiser.process(frame),
            Self::DeepFilter(denoiser) => denoiser.process(frame),
        }
    }

    fn failed(&self) -> bool {
        match self {
            Self::Light(_) => false,
            Self::DeepFilter(denoiser) => denoiser.is_failed(),
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Light(denoiser) => denoiser.reset(),
            Self::DeepFilter(denoiser) => denoiser.reset(),
        }
    }
}

/// Load model files and create ONNX sessions away from the DSP worker. The
/// worker keeps producing RNNoise audio while network I/O/session creation runs.
fn start_deepfilter_loader(
    result_sender: mpsc::SyncSender<Result<ActiveDenoiser>>,
) -> Result<bool> {
    if !crate::resources::can_load_neural_model() {
        return Ok(false);
    }
    if DEEPFILTER_LOAD_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(false);
    }

    let spawn_result = thread::Builder::new()
        .name("deepfilter-loader".into())
        .spawn(move || {
            let loaded = catch_unwind(AssertUnwindSafe(|| {
                let mut loaded = ActiveDenoiser::load(ProcessingMode::Balanced)?;
                // Populate ONNX arenas and output buffers before handing the
                // sessions to the streaming worker. Reset streaming state after
                // warmup so silence from initialization cannot enter live audio.
                for _ in 0..5 {
                    loaded.process(&mut [0.0; FRAME_SIZE]);
                }
                anyhow::ensure!(!loaded.failed(), "DeepFilterNet warmup failed");
                loaded.reset();
                Ok(loaded)
            }))
            .unwrap_or_else(|_| Err(anyhow!("DeepFilterNet loader panicked")));
            let _ = result_sender.try_send(loaded);
            DEEPFILTER_LOAD_IN_PROGRESS.store(false, Ordering::Release);
        });

    match spawn_result {
        Ok(handle) => {
            drop(handle);
            Ok(true)
        }
        Err(error) => {
            DEEPFILTER_LOAD_IN_PROGRESS.store(false, Ordering::Release);
            Err(error.into())
        }
    }
}

fn run_worker(
    pipeline_id: u64,
    mut capture: ringbuf::HeapCons<CapturedFrame>,
    mut outputs: Vec<OutputProducer>,
    input_rate: u32,
    settings: Arc<RuntimeSettings>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut input_resampler = if (input_rate as i64 - ENGINE_RATE as i64).unsigned_abs() >= 1 {
        Some(StreamResampler::new(
            input_rate as usize,
            ENGINE_RATE as usize,
            FRAME_SIZE,
        )?)
    } else {
        None
    };

    let mut requested_mode = settings.load_mode();
    let mut denoiser = ActiveDenoiser::Light(Denoiser::new());
    let mut cached_deepfilter: Option<ActiveDenoiser> = None;
    let mut active_mode = ProcessingMode::Light;
    let mut load_attempted = false;
    let (load_result_sender, load_result_receiver) = mpsc::sync_channel(1);
    AUDIO_METRICS
        .active_mode
        .store(active_mode.as_u8(), Ordering::Relaxed);

    // These existing DSP processors are used only in Studio mode.
    let mut highpass = HighPassFilter::default_80hz();
    let mut energy_vad = EnergyVad::new();
    let mut gate = NoiseGate::with_settings(
        settings.hard_mode.load(Ordering::Relaxed),
        settings.load_gate_settings(),
    );
    let mut eq = ThreeBandEq::new(ENGINE_RATE as f64, settings.load_eq_settings());
    let mut autogain = AutoGain::new(settings.load_autogain_settings());
    let mut pending = VecDeque::<TimedSample>::with_capacity(MAX_PENDING_SAMPLES);
    let mut previous_dry_frame = [0.0f32; FRAME_SIZE];

    let mut slow_neural_frames = 0u32;
    while !stop.load(Ordering::Acquire) && !crate::resources::STOP_AUDIO.load(Ordering::Acquire) {
        match load_result_receiver.try_recv() {
            Ok(Ok(loaded))
                if requested_mode != ProcessingMode::Light
                    && crate::resources::can_load_neural_model() =>
            {
                denoiser = loaded;
                active_mode = requested_mode;
                load_attempted = requested_mode != ProcessingMode::Light;
                AUDIO_METRICS
                    .active_mode
                    .store(active_mode.as_u8(), Ordering::Relaxed);
                tracing::info!("DeepFilterNet is ready; active mode is {active_mode}");
            }
            Ok(Ok(_loaded)) => {
                // The user switched back to Light while the model was loading.
                // Keep the ready sessions for a later mode switch.
                if crate::resources::can_load_neural_model() {
                    cached_deepfilter = Some(_loaded);
                }
                load_attempted = false;
            }
            Ok(Err(error)) => {
                tracing::error!(
                    "Could not initialize DeepFilterNet; continuing with single-pass RNNoise: {error}"
                );
                active_mode = ProcessingMode::Light;
                load_attempted = requested_mode != ProcessingMode::Light;
                AUDIO_METRICS
                    .active_mode
                    .store(active_mode.as_u8(), Ordering::Relaxed);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                load_attempted = requested_mode != ProcessingMode::Light;
            }
        }

        if !crate::resources::can_load_neural_model() {
            settings
                .mode
                .store(ProcessingMode::Light.as_u8(), Ordering::Relaxed);
            cached_deepfilter = None;
        }
        let next_mode = settings.load_mode();
        if next_mode != requested_mode {
            requested_mode = next_mode;
            tracing::info!("Changing processing mode to {requested_mode}");
            if requested_mode == ProcessingMode::Light {
                if matches!(&denoiser, ActiveDenoiser::DeepFilter(_)) {
                    let active =
                        std::mem::replace(&mut denoiser, ActiveDenoiser::Light(Denoiser::new()));
                    if crate::resources::can_load_neural_model() {
                        cached_deepfilter = Some(active);
                    }
                }
                active_mode = ProcessingMode::Light;
                load_attempted = false;
                slow_neural_frames = 0;
            } else if let Some(mut cached) = cached_deepfilter.take() {
                cached.reset();
                denoiser = cached;
                active_mode = requested_mode;
                load_attempted = true;
            } else if matches!(&denoiser, ActiveDenoiser::DeepFilter(_)) {
                // Balanced and Studio share the same stateful DeepFilterNet
                // processor; only the post-processing chain changes.
                active_mode = requested_mode;
            }
            AUDIO_METRICS
                .active_mode
                .store(active_mode.as_u8(), Ordering::Relaxed);
        }

        if requested_mode != ProcessingMode::Light
            && !matches!(&denoiser, ActiveDenoiser::DeepFilter(_))
            && !load_attempted
            && crate::resources::can_load_neural_model()
        {
            match start_deepfilter_loader(load_result_sender.clone()) {
                Ok(started) => load_attempted = started,
                Err(error) => {
                    tracing::error!("Could not start DeepFilterNet loader: {error}");
                    load_attempted = true;
                }
            }
        }

        let Some(block) = capture.try_pop() else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };

        let resampled: &[f32] = match input_resampler.as_mut() {
            Some(resampler) => resampler.process(&block.samples),
            None => &block.samples,
        };
        if !resampled.is_empty() {
            let rms = (resampled.iter().map(|sample| sample * sample).sum::<f32>()
                / resampled.len() as f32)
                .sqrt();
            AUDIO_LEVEL.store(rms.to_bits(), Ordering::Relaxed);
        }

        for (index, &sample) in resampled.iter().enumerate() {
            if pending.len() == MAX_PENDING_SAMPLES {
                pending.pop_front();
                AUDIO_METRICS
                    .input_overrun_frames
                    .fetch_add(1, Ordering::Relaxed);
            }
            let offset_us = (index as u64).saturating_mul(1_000_000) / ENGINE_RATE as u64;
            pending.push_back(TimedSample {
                sample,
                captured_at: block.captured_at + Duration::from_micros(offset_us),
            });
        }

        while pending.len() >= FRAME_SIZE && !stop.load(Ordering::Acquire) {
            let mut frame = [0.0f32; FRAME_SIZE];
            let mut raw_frame = [0.0f32; FRAME_SIZE];
            let mut dry_frame = [0.0f32; FRAME_SIZE];
            let mut frame_captured_at = None;
            for sample in &mut frame {
                if let Some(timed) = pending.pop_front() {
                    *sample = timed.sample;
                    frame_captured_at.get_or_insert(timed.captured_at);
                }
            }
            let frame_captured_at = frame_captured_at.unwrap_or_else(Instant::now);
            raw_frame.copy_from_slice(&frame);
            let bypass = !settings.denoise_enabled.load(Ordering::Relaxed);
            let studio =
                requested_mode == ProcessingMode::Studio && active_mode == ProcessingMode::Studio;

            let process_started = Instant::now();
            if studio {
                highpass.process(&mut frame);
            }
            dry_frame.copy_from_slice(&frame);

            // Run one neural suppressor per frame. In A/B bypass the model state
            // continues advancing, while raw input is sent to the output.
            let dry_delay_samples = match &denoiser {
                ActiveDenoiser::Light(_) => RNNOISE_DRY_DELAY_SAMPLES,
                ActiveDenoiser::DeepFilter(_) => DEEPFILTER_DRY_DELAY_SAMPLES,
            };
            let result = denoiser.process(&mut frame);
            if denoiser.failed() {
                tracing::error!("DeepFilterNet failed repeatedly; switching to RNNoise");
                denoiser = ActiveDenoiser::Light(Denoiser::new());
                active_mode = ProcessingMode::Light;
                AUDIO_METRICS
                    .active_mode
                    .store(active_mode.as_u8(), Ordering::Relaxed);
            }

            if bypass {
                frame.copy_from_slice(&raw_frame);
            } else {
                let suppression =
                    f32::from_bits(settings.suppression_level.load(Ordering::Relaxed))
                        .clamp(0.0, 1.0);
                if suppression <= 0.0 {
                    frame.copy_from_slice(&dry_frame);
                } else if suppression < 1.0 {
                    blend_dry_wet_aligned(
                        &previous_dry_frame,
                        &dry_frame,
                        &mut frame,
                        suppression,
                        dry_delay_samples,
                    );
                }

                if studio && active_mode == ProcessingMode::Studio {
                    if let Some(vad) = result.vad {
                        gate.set_vad(vad);
                    } else if let Some(vad) = energy_vad.process(&mut frame).vad {
                        gate.set_vad(vad);
                    }
                    gate.set_enabled(
                        settings.hard_mode.load(Ordering::Relaxed)
                            || settings.gate_floor.load(Ordering::Relaxed) != 1.0f32.to_bits(),
                    );
                    gate.update_settings(settings.load_gate_settings());
                    eq.update_settings(settings.load_eq_settings());
                    autogain.update_settings(settings.load_autogain_settings());
                    gate.process(&mut frame);
                    eq.process(&mut frame);
                    autogain.process(&mut frame);
                }
            }
            previous_dry_frame.copy_from_slice(&dry_frame);

            let output_rms = (frame.iter().map(|sample| sample * sample).sum::<f32>()
                / FRAME_SIZE as f32)
                .sqrt();
            OUTPUT_LEVEL.store(output_rms.to_bits(), Ordering::Relaxed);

            for output in &mut outputs {
                let samples: &[f32] = match output.resampler.as_mut() {
                    Some(resampler) => resampler.process(&frame),
                    None => &frame,
                };
                for &sample in samples {
                    let timed = TimedSample {
                        sample: if sample.is_finite() {
                            sample.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        },
                        captured_at: frame_captured_at,
                    };
                    if output.producer.try_push(timed).is_err() {
                        AUDIO_METRICS
                            .output_overrun_samples
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            let elapsed = process_started.elapsed();
            AUDIO_METRICS.record_frame_time(elapsed);
            if neural_budget_exceeded(
                matches!(&denoiser, ActiveDenoiser::DeepFilter(_)),
                elapsed,
                &mut slow_neural_frames,
            ) {
                tracing::warn!(
                    "Neural processing exceeded 8 ms for 30 consecutive frames; falling back to RNNoise"
                );
                settings
                    .mode
                    .store(ProcessingMode::Light.as_u8(), Ordering::Relaxed);
                slow_neural_frames = 0;
            }
        }
    }

    if PIPELINE_ID.load(Ordering::Acquire) == pipeline_id {
        AUDIO_METRICS
            .active_mode
            .store(NO_ACTIVE_MODE, Ordering::Relaxed);
    }
    Ok(())
}

fn neural_budget_exceeded(neural: bool, elapsed: Duration, consecutive: &mut u32) -> bool {
    if neural && elapsed > Duration::from_millis(8) {
        *consecutive = consecutive.saturating_add(1);
    } else {
        *consecutive = 0;
    }
    *consecutive >= 30
}

/// Mix a dry frame with neural output after compensating the model's streaming
/// delay. `wet` is modified in place; all buffers are fixed-size stack arrays.
fn blend_dry_wet_aligned(
    previous_dry: &[f32; FRAME_SIZE],
    current_dry: &[f32; FRAME_SIZE],
    wet: &mut [f32; FRAME_SIZE],
    wet_amount: f32,
    delay_samples: usize,
) {
    debug_assert!(delay_samples <= FRAME_SIZE);
    let wet_amount = wet_amount.clamp(0.0, 1.0);
    let dry_amount = 1.0 - wet_amount;

    for (index, wet_sample) in wet.iter_mut().enumerate() {
        let dry_index = index as isize - delay_samples as isize;
        let dry_sample = if dry_index < 0 {
            previous_dry[(FRAME_SIZE as isize + dry_index) as usize]
        } else {
            current_dry[dry_index as usize]
        };
        *wet_sample = dry_sample * dry_amount + *wet_sample * wet_amount;
    }
}

pub struct WatchdogHandle {
    running: Arc<AtomicBool>,
    _handle: Option<JoinHandle<()>>,
}

impl WatchdogHandle {
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for WatchdogHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self._handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_out_worker_cannot_admit_a_second_dsp_worker() {
        let first = WorkerAdmission::acquire().unwrap();
        assert!(WorkerAdmission::acquire().is_err());
        drop(first);
        let restarted = WorkerAdmission::acquire().unwrap();
        drop(restarted);
    }

    #[test]
    fn ui_requests_coalesce_without_waiting_for_driver_control() {
        let pipeline = Pipeline::new(Arc::new(RuntimeSettings::from_config(&Config::default())));
        // Simulate a controller occupied inside driver initialization.
        let _busy_controller = lock_or_recover(&pipeline.control);
        for i in 0..10_000 {
            pipeline.request_running(i % 2 == 0);
        }
        pipeline.request_devices(None, Some("monitor-id"), Some("saved-virtual-id"));
        assert_eq!(*lock_or_recover(&pipeline.pending_control), Some(false));
        let saved = pipeline.to_config(&Config::default());
        assert_eq!(saved.input_device, None);
        assert_eq!(saved.virtual_device.as_deref(), Some("saved-virtual-id"));
        let receiver = lock_or_recover(&pipeline.control_receiver).take().unwrap();
        assert!(receiver.try_recv().is_ok());
        assert!(
            receiver.try_recv().is_err(),
            "one wake slot; requests must not grow a queue"
        );
    }

    #[test]
    fn neural_budget_tolerates_a_spike_and_trips_on_sustained_overload() {
        let mut consecutive = 0;
        assert!(!neural_budget_exceeded(
            true,
            Duration::from_millis(100),
            &mut consecutive
        ));
        assert!(!neural_budget_exceeded(
            true,
            Duration::from_millis(1),
            &mut consecutive
        ));
        for _ in 0..29 {
            assert!(!neural_budget_exceeded(
                true,
                Duration::from_millis(9),
                &mut consecutive
            ));
        }
        assert!(neural_budget_exceeded(
            true,
            Duration::from_millis(9),
            &mut consecutive
        ));
        assert!(!neural_budget_exceeded(
            false,
            Duration::from_millis(100),
            &mut consecutive
        ));
    }

    fn normalized_correlation(a: &[f32], b: &[f32]) -> f64 {
        let (mut xy, mut xx, mut yy) = (0.0f64, 0.0f64, 0.0f64);
        for (&x, &y) in a.iter().zip(b) {
            let x = f64::from(x);
            let y = f64::from(y);
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        xy / (xx * yy).sqrt().max(f64::MIN_POSITIVE)
    }

    #[test]
    fn capture_downmixes_stereo_f32_into_480_sample_frames() {
        let ring = ringbuf::HeapRb::<CapturedFrame>::new(2);
        let (producer, mut consumer) = ring.split();
        let mut capture = CaptureWriter::new(producer, 2, ENGINE_RATE);
        let mut interleaved = Vec::with_capacity(FRAME_SIZE * 2);
        for _ in 0..FRAME_SIZE {
            interleaved.extend_from_slice(&[0.25, -0.25]);
        }
        capture.capture_f32(&interleaved, Instant::now(), 1.0);
        let frame = consumer.try_pop().expect("one full frame should be queued");
        assert_eq!(frame.samples.len(), FRAME_SIZE);
        assert!(
            frame
                .samples
                .iter()
                .all(|sample| sample.abs() < f32::EPSILON)
        );
    }

    #[test]
    fn capture_converts_i16_input_to_normalized_float() {
        let ring = ringbuf::HeapRb::<CapturedFrame>::new(2);
        let (producer, mut consumer) = ring.split();
        let mut capture = CaptureWriter::new(producer, 1, ENGINE_RATE);
        capture.capture_i16(&[i16::MIN; FRAME_SIZE], Instant::now(), 1.0);
        let frame = consumer.try_pop().expect("one full frame should be queued");
        assert!(frame.samples.iter().all(|sample| *sample == -1.0));
    }

    #[test]
    fn i16_output_conversion_handles_both_endpoints() {
        assert_eq!(to_i16(-1.0), i16::MIN);
        assert_eq!(to_i16(1.0), i16::MAX);
        assert_eq!(to_i16(f32::NAN), 0);
    }

    #[test]
    fn output_reader_waits_for_a_20ms_prefill_before_starting() {
        let settings = Arc::new(RuntimeSettings::from_config(&Config::default()));
        let prefill_samples = ENGINE_RATE as usize * OUTPUT_PREFILL_MS / 1000;
        let ring = ringbuf::HeapRb::<TimedSample>::new(prefill_samples * 2);
        let (mut producer, consumer) = ring.split();
        let mut reader = OutputReader::new(
            consumer,
            2,
            PIPELINE_ID.load(Ordering::Acquire),
            false,
            settings,
            ENGINE_RATE,
        );

        assert!(!reader.prime_if_ready(FRAME_SIZE));
        for _ in 0..prefill_samples - 1 {
            assert!(
                producer
                    .try_push(TimedSample {
                        sample: 0.0,
                        captured_at: Instant::now(),
                    })
                    .is_ok(),
                "prefill ring has enough capacity"
            );
        }
        assert!(!reader.prime_if_ready(FRAME_SIZE));
        assert!(
            producer
                .try_push(TimedSample {
                    sample: 0.0,
                    captured_at: Instant::now(),
                })
                .is_ok(),
            "prefill ring has enough capacity"
        );
        assert!(reader.prime_if_ready(FRAME_SIZE));
        assert!(reader.prime_if_ready(FRAME_SIZE));
    }

    #[test]
    fn output_prefill_accounts_for_large_wasapi_callbacks_and_recovers() {
        let settings = Arc::new(RuntimeSettings::from_config(&Config::default()));
        let ring = ringbuf::HeapRb::<TimedSample>::new(OUTPUT_QUEUE_SAMPLES);
        let (mut producer, consumer) = ring.split();
        let mut reader = OutputReader::new(consumer, 2, 0, false, settings, ENGINE_RATE);
        let push = |producer: &mut ringbuf::HeapProd<TimedSample>, count| {
            for _ in 0..count {
                assert!(
                    producer
                        .try_push(TimedSample {
                            sample: 0.2,
                            captured_at: Instant::now(),
                        })
                        .is_ok()
                );
            }
        };
        push(&mut producer, 960);
        assert!(!reader.prime_if_ready(1056));
        assert_eq!(reader.prefill_samples, 1536);
        assert_eq!(reader.consumer.occupied_len(), 960);
        push(&mut producer, 576);
        assert!(reader.prime_if_ready(1056));
        for _ in 0..1056 {
            reader.consumer.try_pop().unwrap();
        }
        assert_eq!(reader.consumer.occupied_len(), 480);
        // A late producer genuinely underruns, then subsequent callbacks wait
        // for a complete prefill instead of repeatedly emitting partial audio.
        reader.record_callback(1, None);
        assert!(!reader.prime_if_ready(1056));
        push(&mut producer, 1056);
        assert!(reader.prime_if_ready(1056));
        assert!(reader.prefill_samples <= OUTPUT_QUEUE_SAMPLES);
    }

    #[test]
    fn output_clock_interpolation_is_continuous_and_bounded() {
        let settings = Arc::new(RuntimeSettings::from_config(&Config::default()));
        let ring = ringbuf::HeapRb::<TimedSample>::new(OUTPUT_QUEUE_SAMPLES);
        let (mut producer, consumer) = ring.split();
        let mut reader = OutputReader::new(consumer, 1, 0, false, settings, ENGINE_RATE);
        let at = Instant::now();
        for i in 0..100 {
            assert!(
                producer
                    .try_push(TimedSample {
                        sample: i as f32 * 0.001,
                        captured_at: at
                    })
                    .is_ok()
            );
        }
        reader.rate_ratio = 0.998;
        for i in 0..60 {
            let sample = reader.next_output().unwrap();
            assert!((sample.sample - i as f32 * 0.998 * 0.001).abs() < 1e-6);
        }
        reader.update_clock_ratio(48);
        assert!((0.998..=1.002).contains(&reader.rate_ratio));
    }

    #[test]
    fn independent_output_clocks_remain_continuous_for_five_minutes() {
        // Simulate +/- 1000 ppm device drift with 480-sample capture packets,
        // 1056-frame render callbacks, and no sleeps or real audio devices.
        for drift_ppm in [-1000.0, 1000.0] {
            let settings = Arc::new(RuntimeSettings::from_config(&Config::default()));
            let ring = ringbuf::HeapRb::<TimedSample>::new(OUTPUT_QUEUE_SAMPLES);
            let (mut producer, consumer) = ring.split();
            let mut reader = OutputReader::new(
                consumer,
                1,
                PIPELINE_ID.load(Ordering::Acquire),
                false,
                settings,
                ENGINE_RATE,
            );
            let at = Instant::now();
            let packet = TimedSample {
                sample: 0.2,
                captured_at: at,
            };
            for _ in 0..1920 {
                assert!(producer.try_push(packet).is_ok());
            }
            let mut fractional_input = 0.0;
            let mut rendered = [0.0; 1056];
            for callback in 0..(300 * ENGINE_RATE as usize / 1056) {
                if callback > 0 {
                    fractional_input += 1056.0 * (1.0 + drift_ppm / 1_000_000.0);
                    while fractional_input >= FRAME_SIZE as f64 {
                        for _ in 0..FRAME_SIZE {
                            assert!(
                                producer.try_push(packet).is_ok(),
                                "overrun at {drift_ppm} ppm"
                            );
                        }
                        fractional_input -= FRAME_SIZE as f64;
                    }
                }
                reader.render_f32(&mut rendered);
                assert!(
                    reader.primed,
                    "underrun at callback {callback}, {drift_ppm} ppm"
                );
                assert!(rendered.iter().all(|x| (*x - 0.2).abs() < 1e-6));
                assert!((0.998..=1.002).contains(&reader.rate_ratio));
            }
            assert!((reader.rate_ratio - (1.0 + drift_ppm / 1_000_000.0)).abs() < 0.00015);
        }
    }

    #[test]
    fn latency_average_and_max_are_monotonic_for_test_samples() {
        let metrics = AudioMetrics::new();
        metrics.record_latency(Duration::from_millis(5));
        metrics.record_latency(Duration::from_millis(10));
        assert_eq!(metrics.latency_last_us.load(Ordering::Relaxed), 10_000);
        assert_eq!(metrics.latency_max_us.load(Ordering::Relaxed), 10_000);
        assert!(metrics.latency_average_us.load(Ordering::Relaxed) >= 5_000);
    }

    #[test]
    fn rnnoise_dry_wet_mix_compensates_streaming_delay() {
        let mut denoiser = Denoiser::new();
        const FRAME_COUNT: usize = 48;
        let mut previous_dry = [0.0f32; FRAME_SIZE];
        let mut input = Vec::with_capacity(FRAME_COUNT * FRAME_SIZE);
        let mut wet_output = Vec::with_capacity(FRAME_COUNT * FRAME_SIZE);
        let mut unaligned_mix = Vec::with_capacity(FRAME_COUNT * FRAME_SIZE);
        let mut aligned_mix = Vec::with_capacity(FRAME_COUNT * FRAME_SIZE);
        let mut phase = 0.0f32;

        for frame_index in 0..FRAME_COUNT {
            let mut dry = [0.0f32; FRAME_SIZE];
            for (sample_index, sample) in dry.iter_mut().enumerate() {
                let absolute = frame_index * FRAME_SIZE + sample_index;
                let time = absolute as f32 / ENGINE_RATE as f32;
                let frequency = 135.0 + 18.0 * (std::f32::consts::TAU * 0.7 * time).sin();
                phase += std::f32::consts::TAU * frequency / ENGINE_RATE as f32;
                let envelope =
                    0.12 + 0.06 * (0.5 + 0.5 * (std::f32::consts::TAU * 3.0 * time).sin());
                *sample = envelope
                    * (phase.sin()
                        + 0.35 * (2.0 * phase).sin()
                        + 0.15 * (3.0 * phase).sin()
                        + 0.05 * (4.0 * phase).sin());
            }

            input.extend_from_slice(&dry);
            let mut wet = dry;
            denoiser.process(&mut wet);
            wet_output.extend_from_slice(&wet);

            let mut unaligned = wet;
            for (sample, &dry_sample) in unaligned.iter_mut().zip(&dry) {
                *sample = dry_sample * 0.25 + *sample * 0.75;
            }
            unaligned_mix.extend_from_slice(&unaligned);

            let mut aligned = wet;
            blend_dry_wet_aligned(
                &previous_dry,
                &dry,
                &mut aligned,
                0.75,
                RNNOISE_DRY_DELAY_SAMPLES,
            );
            aligned_mix.extend_from_slice(&aligned);
            previous_dry = dry;
        }

        let start = FRAME_SIZE * 4;
        let end = input.len() - FRAME_SIZE * 4;
        let delay = RNNOISE_DRY_DELAY_SAMPLES;
        let target = &input[start - delay..end - delay];
        assert!(
            normalized_correlation(target, &aligned_mix[start..end])
                > normalized_correlation(target, &unaligned_mix[start..end]) + 0.04,
            "delay-aligned dry/wet mixing should avoid the cancellation measured in the original mix"
        );
        assert!(normalized_correlation(target, &wet_output[start..end]) > 0.98);
    }
}
