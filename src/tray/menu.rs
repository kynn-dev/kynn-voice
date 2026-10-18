use std::sync::Arc;
use std::sync::atomic::Ordering;

use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use winit::event_loop::ActiveEventLoop;

use crate::audio::device;
use crate::audio::pipeline::Pipeline;
use crate::config::{Config, ProcessingMode};

/// Holds references to menu items we need to update dynamically.
pub struct MenuState {
    pub toggle_id: muda::MenuId,
    pub denoise_id: muda::MenuId,
    pub eq_enabled_id: muda::MenuId,
    pub settings_id: muda::MenuId,
    pub quit_id: muda::MenuId,
    pub input_items: Vec<(CheckMenuItem, String)>,
    pub output_items: Vec<(CheckMenuItem, String)>,
    pub mode_light_id: muda::MenuId,
    pub mode_balanced_id: muda::MenuId,
    pub mode_studio_id: muda::MenuId,
    pub mode_light_item: CheckMenuItem,
    pub mode_balanced_item: CheckMenuItem,
    pub mode_studio_item: CheckMenuItem,
}

pub fn build_menu(config: &Config, pipeline: &Arc<Pipeline>) -> (Menu, MenuState) {
    let menu = Menu::new();

    // ── Toggle on/off ───────────────────────────────────────────────
    let is_running = pipeline.wants_to_run();
    let toggle = MenuItem::new(if is_running { "Stop" } else { "Start" }, true, None);
    let toggle_id = toggle.id().clone();
    let _ = menu.append(&toggle);

    let _ = menu.append(&PredefinedMenuItem::separator());

    // ── Input device submenu ────────────────────────────────────────
    let input_sub = Submenu::new("Input Device", true);
    let devices = device::cached_devices();
    let mut input_items = Vec::new();
    // Empty name follows the Windows/system default input.
    let sys_default_input = CheckMenuItem::new(
        "Dispositivo de entrada padrão do Windows",
        true,
        config.input_device.is_none(),
        None,
    );
    input_items.push((sys_default_input.clone(), String::new()));
    let _ = input_sub.append(&sys_default_input);
    let _ = input_sub.append(&PredefinedMenuItem::separator());

    {
        for dev in &devices.inputs {
            let is_selected = config.input_device.as_deref().is_some_and(|s| s == dev.id);
            let item = CheckMenuItem::new(&dev.display_name(), true, is_selected, None);
            input_items.push((item.clone(), dev.id.clone()));
            let _ = input_sub.append(&item);
        }
    }
    let _ = menu.append(&input_sub);

    // ── Output device submenu ───────────────────────────────────────
    let output_sub = Submenu::new("Monitor Output", true);
    let mut output_items = Vec::new();
    let none_item = CheckMenuItem::new(
        "None (no monitoring)",
        true,
        config.output_device.is_none(),
        None,
    );
    output_items.push((none_item.clone(), String::new()));
    let _ = output_sub.append(&none_item);

    {
        for dev in &devices.outputs {
            let is_selected = config.output_device.as_deref().is_some_and(|s| s == dev.id);
            let item = CheckMenuItem::new(&dev.display_name(), true, is_selected, None);
            output_items.push((item.clone(), dev.id.clone()));
            let _ = output_sub.append(&item);
        }
    }
    let _ = menu.append(&output_sub);

    let _ = menu.append(&PredefinedMenuItem::separator());

    // ── DSP toggles ─────────────────────────────────────────────────
    let denoise = CheckMenuItem::new(
        "Supressão ligada (desmarcar = bypass A/B)",
        true,
        pipeline.settings.denoise_enabled.load(Ordering::Relaxed),
        None,
    );
    let denoise_id = denoise.id().clone();
    let _ = menu.append(&denoise);

    let eq_enabled = CheckMenuItem::new(
        "EQ (warmth correction)",
        true,
        pipeline.settings.eq_enabled.load(Ordering::Relaxed),
        None,
    );
    let eq_enabled_id = eq_enabled.id().clone();
    let _ = menu.append(&eq_enabled);

    let _ = menu.append(&PredefinedMenuItem::separator());

    // ── Processing mode selection ────────────────────────────────────
    let mode_sub = Submenu::new("Processing Mode", true);
    let mode_light_item = CheckMenuItem::new(
        "Leve (RNNoise)",
        true,
        config.mode == ProcessingMode::Light,
        None,
    );
    let mode_light_id = mode_light_item.id().clone();
    let _ = mode_sub.append(&mode_light_item);
    let mode_balanced_item = CheckMenuItem::new(
        "Equilibrado (DeepFilterNet)",
        true,
        config.mode == ProcessingMode::Balanced,
        None,
    );
    let mode_balanced_id = mode_balanced_item.id().clone();
    let _ = mode_sub.append(&mode_balanced_item);
    let mode_studio_item = CheckMenuItem::new(
        "Estúdio (DeepFilterNet + voz)",
        true,
        config.mode == ProcessingMode::Studio,
        None,
    );
    let mode_studio_id = mode_studio_item.id().clone();
    let _ = mode_sub.append(&mode_studio_item);
    let _ = menu.append(&mode_sub);

    let _ = menu.append(&PredefinedMenuItem::separator());

    // ── Settings window ────────────────────────────────────────────
    let settings_item = MenuItem::new("Settings...", true, None);
    let settings_id = settings_item.id().clone();
    let _ = menu.append(&settings_item);

    // ── Quit ────────────────────────────────────────────────────────
    let quit = MenuItem::new("Quit", true, None);
    let quit_id = quit.id().clone();
    let _ = menu.append(&quit);

    let state = MenuState {
        toggle_id,
        denoise_id,
        eq_enabled_id,
        settings_id,
        quit_id,
        input_items,
        output_items,
        mode_light_id,
        mode_balanced_id,
        mode_studio_id,
        mode_light_item,
        mode_balanced_item,
        mode_studio_item,
    };

    (menu, state)
}

/// Set mode checkmarks — ensures exactly one is checked.
fn set_mode_checked(state: &MenuState, mode: ProcessingMode) {
    state
        .mode_light_item
        .set_checked(mode == ProcessingMode::Light);
    state
        .mode_balanced_item
        .set_checked(mode == ProcessingMode::Balanced);
    state
        .mode_studio_item
        .set_checked(mode == ProcessingMode::Studio);
}

fn select_mode(
    state: &MenuState,
    pipeline: &Arc<Pipeline>,
    config: &mut Config,
    mode: ProcessingMode,
) {
    config.mode = mode;
    pipeline
        .settings
        .mode
        .store(mode.as_u8(), Ordering::Relaxed);
    set_mode_checked(state, mode);
    tracing::info!("Processing mode selected: {mode}");
}

pub fn handle_event(
    event: &MenuEvent,
    state: &MenuState,
    pipeline: &Arc<Pipeline>,
    config: &mut Config,
    event_loop: &ActiveEventLoop,
) {
    let id = event.id();

    // ── Quit ────────────────────────────────────────────────────────
    if *id == state.quit_id {
        tracing::info!("Quit requested");
        // Quitting is not the same as pausing. Preserve the user's desired
        // audio state for the next launch; main handles final flush and stop.
        event_loop.exit();
        return;
    }

    // ── Toggle pipeline ─────────────────────────────────────────────
    if *id == state.toggle_id {
        if pipeline.wants_to_run() {
            pipeline.request_running(false);
        } else {
            pipeline.request_running(true);
        }
        return;
    }

    // ── Denoise toggle ──────────────────────────────────────────────
    if *id == state.denoise_id {
        let current = pipeline.settings.denoise_enabled.load(Ordering::Relaxed);
        pipeline
            .settings
            .denoise_enabled
            .store(!current, Ordering::Relaxed);
        tracing::info!("Denoise: {}", !current);
        return;
    }

    // ── Settings window ────────────────────────────────────────────
    // Handled by TrayApp via the open_settings flag — see tray/mod.rs

    // ── EQ toggle ───────────────────────────────────────────────────
    if *id == state.eq_enabled_id {
        let current = pipeline.settings.eq_enabled.load(Ordering::Relaxed);
        pipeline
            .settings
            .eq_enabled
            .store(!current, Ordering::Relaxed);
        tracing::info!("EQ: {}", !current);
        return;
    }

    // ── Processing mode selection ────────────────────────────────────
    if *id == state.mode_light_id {
        select_mode(state, pipeline, config, ProcessingMode::Light);
        return;
    }
    if *id == state.mode_balanced_id {
        select_mode(state, pipeline, config, ProcessingMode::Balanced);
        return;
    }
    if *id == state.mode_studio_id {
        select_mode(state, pipeline, config, ProcessingMode::Studio);
        return;
    }

    // ── Input device selection ──────────────────────────────────────
    for (item, device_name) in &state.input_items {
        if *id == *item.id() {
            // Empty name = system default input → config None
            config.input_device = if device_name.is_empty() {
                None
            } else {
                Some(device_name.clone())
            };
            pipeline.request_devices(
                config.input_device.as_deref(),
                config.output_device.as_deref(),
                config.virtual_device.as_deref(),
            );
            // Enforce mutual exclusivity: uncheck all, check selected
            for (other, _) in &state.input_items {
                other.set_checked(*other.id() == *item.id());
            }
            tracing::info!(
                "Input device: {}",
                config.input_device.as_deref().unwrap_or("system default")
            );
            return;
        }
    }

    // ── Output device selection ─────────────────────────────────────
    for (item, device_name) in &state.output_items {
        if *id == *item.id() {
            config.output_device = if device_name.is_empty() {
                None
            } else {
                Some(device_name.clone())
            };
            pipeline.request_devices(
                config.input_device.as_deref(),
                config.output_device.as_deref(),
                config.virtual_device.as_deref(),
            );
            // Enforce mutual exclusivity: uncheck all, check selected
            for (other, _) in &state.output_items {
                other.set_checked(*other.id() == *item.id());
            }
            tracing::info!(
                "Monitor output: {}",
                config.output_device.as_deref().unwrap_or("none")
            );
            return;
        }
    }
}
