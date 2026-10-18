//! USER32/common controls on Windows: no GL context or GPU render loop.
use crate::audio::{
    device,
    pipeline::{AUDIO_LEVEL, AUDIO_METRICS, OUTPUT_LEVEL, Pipeline},
};
use crate::config::{Config, ProcessingMode, RuntimeSettings};
use crate::resources;
use anyhow::Context;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{Arc, atomic::Ordering, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, DEFAULT_GUI_FONT, FillRect, GetStockObject, GetSysColorBrush, HDC,
};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

const INPUT: u16 = 101;
const MONITOR: u16 = 102;
const MODE: u16 = 103;
const APPLY: u16 = 104;
const REFRESH: u16 = 105;
const DENOISE: u16 = 106;
const SUPPRESSION: u16 = 107;
const GATE: u16 = 108;
const SENSITIVITY: u16 = 109;
const EQ: u16 = 110;
const CLARITY: u16 = 111;
const BASS: u16 = 112;
const MID: u16 = 113;
const TREBLE: u16 = 114;
const AUTOGAIN: u16 = 115;
const INPUT_GAIN: u16 = 116;
const OUTPUT_GAIN: u16 = 117;
const STATUS: u16 = 118;
const METRICS: u16 = 119;
const RESOURCES: u16 = 120;
const METERS: u16 = 121;
const RESET_EQ: u16 = 122;
const QUIT: u16 = 123;
const STARTUP: u16 = 124;
const SAVE_STATUS: u16 = 125;
const SUBCLASS_ID: usize = 0x4b59564f;
// TBM_GETPOS is WM_USER in commctrl.h; windows-rs does not export the alias.
const TBM_GETPOS: u32 = WM_USER;

#[derive(Clone, Copy)]
enum NativeEvent {
    Command(u16, u16),
    Slider(u16),
}
struct EventSink {
    sender: mpsc::SyncSender<NativeEvent>,
}

// The boxed sink has a stable address until the subclass is removed.
// No driver call, model loading, or pipeline lock occurs inside this callback.
unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_ERASEBKGND {
        let mut rect = windows::Win32::Foundation::RECT::default();
        unsafe {
            if GetClientRect(hwnd, &mut rect).is_ok() {
                FillRect(HDC(wp.0 as _), &rect, GetSysColorBrush(COLOR_BTNFACE));
            }
        }
        return LRESULT(1);
    }
    if data != 0 {
        let sink = unsafe { &*(data as *const EventSink) };
        let event = match msg {
            WM_COMMAND => Some(NativeEvent::Command(
                (wp.0 & 0xffff) as u16,
                ((wp.0 >> 16) & 0xffff) as u16,
            )),
            WM_HSCROLL if lp.0 != 0 => {
                Some(NativeEvent::Slider(
                    unsafe { GetDlgCtrlID(HWND(lp.0 as _)) } as u16,
                ))
            }
            _ => None,
        };
        if let Some(event) = event {
            let _ = sink.sender.try_send(event);
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

struct Control {
    id: u16,
    hwnd: HWND,
    bounds: (i32, i32, i32, i32),
    text: String,
}
pub struct SettingsWindowState {
    window: Window,
    hwnd: HWND,
    controls: Vec<Control>,
    _sink: Box<EventSink>,
    events: mpsc::Receiver<NativeEvent>,
    settings: Arc<RuntimeSettings>,
    pipeline: Arc<Pipeline>,
    inputs: Vec<Option<String>>,
    monitors: Vec<Option<String>>,
    selected_input: Option<String>,
    selected_monitor: Option<String>,
    virtual_device: Option<String>,
    catalog_generation: u64,
    last_metrics: Instant,
    exit_requested: bool,
}

impl SettingsWindowState {
    pub fn new(
        el: &ActiveEventLoop,
        settings: Arc<RuntimeSettings>,
        pipeline: Arc<Pipeline>,
        _input: Option<&str>,
        _output: Option<&str>,
    ) -> Option<Self> {
        match Self::create(el, settings, pipeline, true) {
            Ok(state) => Some(state),
            Err(error) => {
                tracing::error!("Native settings window failed: {error}");
                None
            }
        }
    }
    fn create(
        el: &ActiveEventLoop,
        settings: Arc<RuntimeSettings>,
        pipeline: Arc<Pipeline>,
        visible: bool,
    ) -> anyhow::Result<Self> {
        unsafe {
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                // BUTTON/STATIC/COMBOBOX are USER32 classes. Request only the
                // trackbar classes, which also work without a v6 activation context.
                dwICC: ICC_BAR_CLASSES,
            })
            .ok()
            .context("InitCommonControlsEx")?;
        }
        let window = el.create_window(
            Window::default_attributes()
                .with_title("Kynn Voice — Configurações")
                .with_inner_size(winit::dpi::LogicalSize::new(610.0, 720.0))
                .with_resizable(false)
                .with_visible(false),
        )?;
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            anyhow::bail!("Expected Windows handle");
        };
        let hwnd = HWND(handle.hwnd.get() as _);
        let (sender, events) = mpsc::sync_channel(64);
        let sink = Box::new(EventSink { sender });
        unsafe {
            SetWindowSubclass(
                hwnd,
                Some(subclass),
                SUBCLASS_ID,
                (&*sink as *const EventSink) as usize,
            )
            .ok()
            .context("SetWindowSubclass")?;
        }
        let config = pipeline.to_config(&Config::default());
        let mut state = Self {
            window,
            hwnd,
            controls: Vec::with_capacity(48),
            _sink: sink,
            events,
            settings,
            pipeline,
            inputs: Vec::new(),
            monitors: Vec::new(),
            selected_input: config.input_device,
            selected_monitor: config.output_device,
            virtual_device: config.virtual_device,
            catalog_generation: u64::MAX,
            last_metrics: Instant::now() - Duration::from_secs(1),
            exit_requested: false,
        };
        state.build_controls()?;
        tracing::debug!("Native controls created");
        state.populate_devices();
        tracing::debug!("Native device snapshot populated");
        state.sync_controls();
        tracing::debug!("Native controls synchronized");
        state.render();
        tracing::debug!("Native initial metrics updated");
        state.window.set_visible(visible);
        tracing::info!("Settings opened using native Windows controls (no OpenGL)");
        Ok(state)
    }
    fn add(
        &mut self,
        id: u16,
        class: &str,
        text: &str,
        style: u32,
        bounds: (i32, i32, i32, i32),
    ) -> anyhow::Result<()> {
        let scale = self.window.scale_factor();
        let pixel = |v: i32| (v as f64 * scale).round() as i32;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                &HSTRING::from(class),
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                pixel(bounds.0),
                pixel(bounds.1),
                pixel(bounds.2),
                pixel(bounds.3),
                Some(self.hwnd),
                Some(HMENU(id as usize as _)),
                None,
                None,
            )
            .with_context(|| format!("CreateWindowExW class={class} id={id}"))?
        };
        let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        self.controls.push(Control {
            id,
            hwnd,
            bounds,
            text: text.into(),
        });
        Ok(())
    }
    fn label(&mut self, id: u16, text: &str, y: i32, height: i32) -> anyhow::Result<()> {
        self.add(id, "STATIC", text, 0, (12, y, 584, height))
    }
    fn slider(&mut self, id: u16, label: &str, y: i32, min: i32, max: i32) -> anyhow::Result<()> {
        self.add(id + 1000, "STATIC", label, 0, (12, y + 5, 188, 22))?;
        self.add(
            id,
            "msctls_trackbar32",
            "",
            TBS_NOTICKS | WS_TABSTOP.0,
            (200, y, 394, 28),
        )?;
        self.message(id, TBM_SETRANGEMIN, 0, min as isize);
        self.message(id, TBM_SETRANGEMAX, 1, max as isize);
        Ok(())
    }
    fn check(&mut self, id: u16, label: &str, y: i32) -> anyhow::Result<()> {
        self.add(
            id,
            "BUTTON",
            label,
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0,
            (12, y, 584, 24),
        )
    }
    fn build_controls(&mut self) -> anyhow::Result<()> {
        self.add(
            1300,
            "STATIC",
            "Kynn Voice — microfone local",
            0,
            (12, 8, 300, 24),
        )?;
        self.add(
            STARTUP,
            "BUTTON",
            "Iniciar com o Windows",
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0,
            (326, 8, 268, 24),
        )?;
        self.label(METERS, "Entrada / saída", 36, 20)?;
        self.label(METRICS, "Métricas do áudio", 58, 34)?;
        self.label(RESOURCES, "CPU / memória", 94, 20)?;
        self.label(1301, "Entrada:", 121, 20)?;
        self.add(
            INPUT,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0,
            (112, 117, 482, 250),
        )?;
        self.label(1302, "Monitor:", 155, 20)?;
        self.add(
            MONITOR,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0,
            (112, 151, 482, 250),
        )?;
        self.add(
            APPLY,
            "BUTTON",
            "Aplicar dispositivos",
            WS_TABSTOP.0,
            (12, 185, 200, 28),
        )?;
        self.add(
            REFRESH,
            "BUTTON",
            "Atualizar lista",
            WS_TABSTOP.0,
            (224, 185, 180, 28),
        )?;
        self.add(
            QUIT,
            "BUTTON",
            "Sair do Kynn Voice",
            WS_TABSTOP.0,
            (416, 185, 178, 28),
        )?;
        self.label(
            STATUS,
            "A saída virtual permanece no VB-CABLE selecionado.",
            218,
            32,
        )?;
        self.label(1303, "Modo:", 257, 20)?;
        self.add(
            MODE,
            "COMBOBOX",
            "",
            CBS_DROPDOWNLIST as u32 | WS_TABSTOP.0,
            (112, 253, 482, 150),
        )?;
        for mode in [
            ProcessingMode::Light,
            ProcessingMode::Balanced,
            ProcessingMode::Studio,
        ] {
            self.combo_add(MODE, &mode.to_string());
        }
        self.check(DENOISE, "Supressão ligada (desmarque para bypass A/B)", 290)?;
        self.slider(SUPPRESSION, "Redução (%)", 318, 0, 100)?;
        self.slider(INPUT_GAIN, "Ganho de entrada (%)", 348, 0, 400)?;
        self.slider(OUTPUT_GAIN, "Ganho de saída (%)", 378, 0, 400)?;
        self.label(
            1304,
            "Controles vocais abaixo são aplicados no modo Estúdio.",
            412,
            22,
        )?;
        self.check(GATE, "Gate de voz", 438)?;
        self.slider(SENSITIVITY, "Sensibilidade (%)", 464, 0, 100)?;
        self.check(EQ, "Equalização vocal", 496)?;
        self.slider(CLARITY, "Clareza (%)", 523, 0, 100)?;
        self.slider(BASS, "Graves (0,1 dB)", 552, -120, 120)?;
        self.slider(MID, "Médios (0,1 dB)", 581, -120, 120)?;
        self.slider(TREBLE, "Agudos (0,1 dB)", 610, -120, 120)?;
        self.check(AUTOGAIN, "Normalização automática de volume", 642)?;
        self.add(
            RESET_EQ,
            "BUTTON",
            "Restaurar EQ",
            WS_TABSTOP.0,
            (12, 676, 180, 28),
        )?;
        self.add(
            SAVE_STATUS,
            "STATIC",
            "Salvamento automático ativo",
            0,
            (205, 680, 389, 24),
        )?;
        Ok(())
    }
    fn hwnd(&self, id: u16) -> HWND {
        self.controls
            .iter()
            .find(|c| c.id == id)
            .expect("known control")
            .hwnd
    }
    fn message(&self, id: u16, msg: u32, wp: usize, lp: isize) -> isize {
        unsafe { SendMessageW(self.hwnd(id), msg, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
    }
    fn combo_add(&self, id: u16, text: &str) {
        let wide = HSTRING::from(text);
        self.message(id, CB_ADDSTRING, 0, wide.as_ptr() as isize);
    }
    fn checked(&self, id: u16) -> bool {
        self.message(id, BM_GETCHECK, 0, 0) == BST_CHECKED.0 as isize
    }
    fn set_checked(&self, id: u16, check: bool) {
        self.message(id, BM_SETCHECK, usize::from(check), 0);
    }
    fn position(&self, id: u16) -> i32 {
        self.message(id, TBM_GETPOS, 0, 0) as i32
    }
    fn set_position(&self, id: u16, value: i32) {
        self.message(id, TBM_SETPOS, 1, value as isize);
    }
    fn text(&mut self, id: u16, text: String) {
        let c = self
            .controls
            .iter_mut()
            .find(|c| c.id == id)
            .expect("known control");
        if c.text != text {
            unsafe {
                let _ = SetWindowTextW(c.hwnd, &HSTRING::from(&text));
            }
            c.text = text;
        }
    }
    fn populate_devices(&mut self) {
        let cached = device::cached_devices();
        if self.catalog_generation == cached.generation {
            return;
        }
        self.catalog_generation = cached.generation;
        self.message(INPUT, CB_RESETCONTENT, 0, 0);
        self.message(MONITOR, CB_RESETCONTENT, 0, 0);
        self.inputs.clear();
        self.monitors.clear();
        self.combo_add(INPUT, "Dispositivo de entrada padrão do Windows");
        self.inputs.push(None);
        self.combo_add(MONITOR, "Sem monitoramento");
        self.monitors.push(None);
        for d in &cached.inputs {
            self.combo_add(INPUT, &d.display_name());
            self.inputs.push(Some(d.id.clone()));
        }
        for d in &cached.outputs {
            self.combo_add(MONITOR, &d.display_name());
            self.monitors.push(Some(d.id.clone()));
        }
        for (id, selected, input) in [
            (INPUT, self.selected_input.clone(), true),
            (MONITOR, self.selected_monitor.clone(), false),
        ] {
            let choices = if input { &self.inputs } else { &self.monitors };
            let index = choices.iter().position(|choice| choice == &selected);
            let index = match index {
                Some(index) => index,
                None => {
                    self.combo_add(id, "Dispositivo selecionado indisponível");
                    let choices = if input {
                        &mut self.inputs
                    } else {
                        &mut self.monitors
                    };
                    choices.push(selected);
                    choices.len() - 1
                }
            };
            self.message(id, CB_SETCURSEL, index, 0);
        }
        if let Some(error) = cached.error {
            self.text(STATUS, format!("Falha ao atualizar dispositivos: {error}"));
        }
    }
    fn sync_controls(&mut self) {
        let s = &self.settings;
        self.message(MODE, CB_SETCURSEL, s.load_mode().as_u8() as usize, 0);
        self.set_checked(DENOISE, s.denoise_enabled.load(Ordering::Relaxed));
        self.set_checked(
            GATE,
            f32::from_bits(s.gate_floor.load(Ordering::Relaxed)) < 1.0,
        );
        self.set_checked(EQ, s.eq_enabled.load(Ordering::Relaxed));
        self.set_checked(AUTOGAIN, s.autogain_enabled.load(Ordering::Relaxed));
        self.set_position(
            SUPPRESSION,
            (f32::from_bits(s.suppression_level.load(Ordering::Relaxed)) * 100.0).round() as i32,
        );
        self.set_position(
            INPUT_GAIN,
            (f32::from_bits(s.input_gain.load(Ordering::Relaxed)) * 100.0).round() as i32,
        );
        self.set_position(
            OUTPUT_GAIN,
            (f32::from_bits(s.output_gain.load(Ordering::Relaxed)) * 100.0).round() as i32,
        );
        self.set_position(
            SENSITIVITY,
            ((0.50 - f32::from_bits(s.gate_threshold.load(Ordering::Relaxed))) / 0.45 * 100.0)
                .round() as i32,
        );
        self.set_position(BASS, s.eq_bass_db10.load(Ordering::Relaxed));
        self.set_position(MID, s.eq_mid_db10.load(Ordering::Relaxed));
        self.set_position(TREBLE, s.eq_treble_db10.load(Ordering::Relaxed));
        self.set_position(CLARITY, s.clarity_percent.load(Ordering::Relaxed) as i32);
        for id in [
            SUPPRESSION,
            INPUT_GAIN,
            OUTPUT_GAIN,
            SENSITIVITY,
            CLARITY,
            BASS,
            MID,
            TREBLE,
        ] {
            self.update_slider_label(id);
        }
    }
    fn slider_changed(&mut self, id: u16) {
        let pos = self.position(id);
        let s = &self.settings;
        match id {
            SUPPRESSION => s
                .suppression_level
                .store((pos as f32 / 100.0).to_bits(), Ordering::Relaxed),
            INPUT_GAIN => s
                .input_gain
                .store((pos as f32 / 100.0).to_bits(), Ordering::Relaxed),
            OUTPUT_GAIN => s
                .output_gain
                .store((pos as f32 / 100.0).to_bits(), Ordering::Relaxed),
            SENSITIVITY => s.gate_threshold.store(
                (0.50 - pos as f32 / 100.0 * 0.45).to_bits(),
                Ordering::Relaxed,
            ),
            BASS => s.eq_bass_db10.store(pos, Ordering::Relaxed),
            MID => s.eq_mid_db10.store(pos, Ordering::Relaxed),
            TREBLE => s.eq_treble_db10.store(pos, Ordering::Relaxed),
            CLARITY => {
                s.clarity_percent.store(pos as u32, Ordering::Relaxed);
                let d = crate::dsp::eq::EqSettings::default();
                for (band, db) in [(BASS, d.bass_db), (MID, d.mid_db), (TREBLE, d.treble_db)] {
                    self.set_position(band, (db * pos as f32 / 10.0) as i32);
                    self.slider_changed(band);
                }
            }
            _ => return,
        }
        self.update_slider_label(id);
    }
    fn update_slider_label(&mut self, id: u16) {
        let pos = self.position(id);
        let (label, suffix) = match id {
            SUPPRESSION => ("Redução", "%"),
            INPUT_GAIN => ("Entrada", "%"),
            OUTPUT_GAIN => ("Saída", "%"),
            SENSITIVITY => ("Sensibilidade", "%"),
            CLARITY => ("Clareza", "%"),
            BASS => ("Graves", "x 0,1 dB"),
            MID => ("Médios", "x 0,1 dB"),
            _ => ("Agudos", "x 0,1 dB"),
        };
        self.text(id + 1000, format!("{label}: {pos} {suffix}"));
    }
    fn command(&mut self, id: u16, notification: u16) {
        if notification == CBN_SELCHANGE as u16 {
            let index = self.message(id, CB_GETCURSEL, 0, 0) as usize;
            match id {
                INPUT => self.selected_input = self.inputs.get(index).cloned().flatten(),
                MONITOR => self.selected_monitor = self.monitors.get(index).cloned().flatten(),
                MODE => self.settings.mode.store(
                    ProcessingMode::from_u8(index as u8).as_u8(),
                    Ordering::Relaxed,
                ),
                _ => {}
            }
        } else if notification == BN_CLICKED as u16 {
            match id {
                APPLY => {
                    self.pipeline.request_devices(
                        self.selected_input.as_deref(),
                        self.selected_monitor.as_deref(),
                        self.virtual_device.as_deref(),
                    );
                    self.text(STATUS, "Aplicando dispositivos em segundo plano...".into());
                }
                REFRESH => {
                    device::request_device_refresh();
                    self.text(
                        STATUS,
                        "Atualizando dispositivos em segundo plano...".into(),
                    );
                }
                DENOISE => self
                    .settings
                    .denoise_enabled
                    .store(self.checked(DENOISE), Ordering::Relaxed),
                GATE => self.settings.gate_floor.store(
                    (if self.checked(GATE) { 0.05f32 } else { 1.0f32 }).to_bits(),
                    Ordering::Relaxed,
                ),
                EQ => self
                    .settings
                    .eq_enabled
                    .store(self.checked(EQ), Ordering::Relaxed),
                AUTOGAIN => self
                    .settings
                    .autogain_enabled
                    .store(self.checked(AUTOGAIN), Ordering::Relaxed),
                RESET_EQ => {
                    self.set_position(CLARITY, 100);
                    self.slider_changed(CLARITY);
                }
                QUIT => self.exit_requested = true,
                STARTUP => {
                    if let Err(error) = crate::startup::request_enabled(self.checked(STARTUP)) {
                        tracing::warn!("Cannot change Windows startup: {error}");
                        self.text(SAVE_STATUS, format!("Falha na inicialização: {error}"));
                    }
                }
                _ => {}
            }
        }
    }
    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }
    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }
    pub fn on_window_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        if matches!(event, winit::event::WindowEvent::ScaleFactorChanged { .. }) {
            let scale = self.window.scale_factor();
            for c in &self.controls {
                let pixel = |v: i32| (v as f64 * scale).round() as i32;
                let (x, y, w, h) = c.bounds;
                unsafe {
                    let _ = MoveWindow(c.hwnd, pixel(x), pixel(y), pixel(w), pixel(h), true);
                }
            }
        }
        false
    }
    pub fn render(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                NativeEvent::Command(id, n) => self.command(id, n),
                NativeEvent::Slider(id) => self.slider_changed(id),
            }
        }
        self.populate_devices();
        if self.last_metrics.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_metrics = Instant::now();
        let startup = crate::startup::snapshot();
        self.set_checked(STARTUP, startup.enabled);
        if let Some(error) = crate::persistence::last_error() {
            self.text(SAVE_STATUS, error);
        } else if let Some(error) = startup.error {
            self.text(SAVE_STATUS, format!("Inicialização: {error}"));
        } else if startup.supported && !startup.ready {
            self.text(
                SAVE_STATUS,
                "Consultando inicialização do Windows...".into(),
            );
        } else {
            self.text(SAVE_STATUS, "Salvamento automático ativo".into());
        }
        let db = |v: &std::sync::atomic::AtomicU32| {
            20.0 * f32::from_bits(v.load(Ordering::Relaxed)).max(1e-6).log10()
        };
        self.text(
            METERS,
            format!(
                "Entrada {:.1} dBFS | Saída {:.1} dBFS",
                db(&AUDIO_LEVEL),
                db(&OUTPUT_LEVEL)
            ),
        );
        let active = AUDIO_METRICS.active_mode.load(Ordering::Relaxed);
        let active = if active == u8::MAX {
            "parado / aguardando".into()
        } else {
            ProcessingMode::from_u8(active).to_string()
        };
        self.text(METRICS,format!("Ativo: {active} | DSP {} µs | latência software {} µs\nUnderruns {} frames | drops entrada {} / saída {}",
            AUDIO_METRICS.frame_last_us.load(Ordering::Relaxed),AUDIO_METRICS.latency_last_us.load(Ordering::Relaxed),
            AUDIO_METRICS.output_underrun_frames.load(Ordering::Relaxed),AUDIO_METRICS.input_overrun_frames.load(Ordering::Relaxed),AUDIO_METRICS.output_overrun_samples.load(Ordering::Relaxed)));
        self.text(
            RESOURCES,
            format!(
                "CPU {:.2}% / limite 15% | memória privada {:.1} / 512 MiB",
                resources::CPU_PERCENT_X100.load(Ordering::Relaxed) as f64 / 100.0,
                resources::PRIVATE_BYTES.load(Ordering::Relaxed) as f64 / 1048576.0
            ),
        );
        if resources::STOP_AUDIO.load(Ordering::Acquire) {
            self.text(
                STATUS,
                "Áudio pausado pela proteção de memória. Reinicie Kynn Voice.".into(),
            );
        } else if resources::SAFE_MODE.load(Ordering::Acquire) {
            self.text(
                STATUS,
                "Proteção de memória: usando RNNoise até reiniciar Kynn Voice.".into(),
            );
        } else if let Some(error) = self.pipeline.last_error() {
            self.text(STATUS, error);
        }
    }
}
impl Drop for SettingsWindowState {
    fn drop(&mut self) {
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(subclass), SUBCLASS_ID);
            for c in self.controls.drain(..) {
                let _ = DestroyWindow(c.hwnd);
            }
        }
    }
}

/// Real native-window regression with no streams or persistent configuration.
pub fn smoke_test() -> anyhow::Result<()> {
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ControlFlow, EventLoop};
    struct Smoke {
        state: Option<SettingsWindowState>,
        pipeline: Arc<Pipeline>,
        count: u32,
        failure: Option<String>,
        warmed: Option<(u64, u32, u32)>,
    }
    impl ApplicationHandler for Smoke {
        fn resumed(&mut self, _el: &ActiveEventLoop) {}
        fn window_event(
            &mut self,
            _el: &ActiveEventLoop,
            _id: WindowId,
            _event: winit::event::WindowEvent,
        ) {
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            self.state.take();
            if self.count == 5 {
                self.warmed = Some((
                    resources::memory_usage().map_or(0, |m| m.0),
                    gui_counts().0,
                    gui_counts().1,
                ));
            }
            if self.count >= 50 {
                el.exit();
                return;
            }
            match SettingsWindowState::create(
                el,
                Arc::clone(&self.pipeline.settings),
                Arc::clone(&self.pipeline),
                false,
            ) {
                Ok(mut state) => {
                    let test = (|| -> anyhow::Result<()> {
                        state.set_position(SUPPRESSION, 37);
                        unsafe {
                            SendMessageW(
                                state.hwnd,
                                WM_HSCROLL,
                                None,
                                Some(LPARAM(state.hwnd(SUPPRESSION).0 as isize)),
                            );
                        }
                        state.render();
                        anyhow::ensure!(
                            (f32::from_bits(
                                state.settings.suppression_level.load(Ordering::Relaxed)
                            ) - 0.37)
                                .abs()
                                < 0.001,
                            "slider notification failed"
                        );
                        state.message(MODE, CB_SETCURSEL, 2, 0);
                        unsafe {
                            SendMessageW(
                                state.hwnd,
                                WM_COMMAND,
                                Some(WPARAM(((CBN_SELCHANGE as usize) << 16) | MODE as usize)),
                                None,
                            );
                        }
                        state.render();
                        anyhow::ensure!(
                            state.settings.load_mode() == ProcessingMode::Studio,
                            "mode notification failed"
                        );
                        state.set_checked(DENOISE, true);
                        state.message(DENOISE, BM_CLICK, 0, 0);
                        state.render();
                        anyhow::ensure!(
                            !state.settings.denoise_enabled.load(Ordering::Relaxed),
                            "native bypass checkbox failed"
                        );
                        state.set_position(BASS, -37);
                        unsafe {
                            SendMessageW(
                                state.hwnd,
                                WM_HSCROLL,
                                None,
                                Some(LPARAM(state.hwnd(BASS).0 as isize)),
                            );
                        }
                        state.render();
                        anyhow::ensure!(
                            state.settings.eq_bass_db10.load(Ordering::Relaxed) == -37,
                            "native EQ slider failed"
                        );
                        state.message(INPUT, CB_SETCURSEL, 0, 0);
                        unsafe {
                            SendMessageW(
                                state.hwnd,
                                WM_COMMAND,
                                Some(WPARAM(((CBN_SELCHANGE as usize) << 16) | INPUT as usize)),
                                None,
                            );
                            SendMessageW(
                                state.hwnd,
                                WM_COMMAND,
                                Some(WPARAM(APPLY as usize)),
                                None,
                            );
                        }
                        state.render();
                        anyhow::ensure!(
                            state
                                .pipeline
                                .to_config(&Config::default())
                                .input_device
                                .is_none(),
                            "default selection was not retained"
                        );
                        anyhow::ensure!(
                            state
                                .pipeline
                                .to_config(&Config::default())
                                .virtual_device
                                .as_deref()
                                == Some("saved-virtual-id"),
                            "device apply changed the saved virtual output"
                        );
                        anyhow::ensure!(
                            !state.pipeline.is_running(),
                            "UI regression must not open audio devices"
                        );
                        Ok(())
                    })();
                    if let Err(e) = test {
                        self.failure = Some(e.to_string());
                        el.exit();
                        return;
                    }
                    self.state = Some(state);
                    self.count += 1;
                }
                Err(e) => {
                    self.failure = Some(e.to_string());
                    el.exit();
                }
            }
            el.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(20)));
        }
    }
    let settings = Arc::new(RuntimeSettings::from_config(&Config::default()));
    let mut app = Smoke {
        state: None,
        pipeline: Arc::new(Pipeline::new(settings)),
        count: 0,
        failure: None,
        warmed: None,
    };
    app.pipeline
        .remember_devices(Some("saved-usb-id"), None, Some("saved-virtual-id"));
    let before = resources::memory_usage()?;
    EventLoop::new()?.run_app(&mut app)?;
    app.state.take();
    anyhow::ensure!(
        app.failure.is_none(),
        "Native settings regression failed: {:?}",
        app.failure
    );
    anyhow::ensure!(app.count == 50, "Incomplete UI regression");
    let after = resources::memory_usage()?;
    let (private_warm, gdi_warm, user_warm) = app.warmed.expect("warmup measurements");
    let (gdi_after, user_after) = gui_counts();
    anyhow::ensure!(
        after.0 <= private_warm + 8 * 1024 * 1024,
        "Native windows grew private memory by more than 8 MiB after warmup"
    );
    anyhow::ensure!(
        gdi_after <= gdi_warm + 4 && user_after <= user_warm + 4,
        "Native window handles are accumulating"
    );
    println!(
        "UI_SMOKE_PASS windows={} slider=PASS mode=PASS bypass=PASS eq=PASS default_input=PASS virtual_output_preserved=PASS audio_streams=0 private_before={} private_after={} private_after_warmup={} working_set_before={} working_set_after={} gdi_warm={} gdi_after={} user_warm={} user_after={}",
        app.count,
        before.0,
        after.0,
        private_warm,
        before.1,
        after.1,
        gdi_warm,
        gdi_after,
        user_warm,
        user_after
    );
    Ok(())
}

fn gui_counts() -> (u32, u32) {
    use windows::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources,
    };
    unsafe {
        (
            GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS),
            GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS),
        )
    }
}
