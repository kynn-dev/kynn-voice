/// Kynn Voice — lightweight real-time microphone noise cancellation.
#[derive(Debug, Default)]
pub struct Cli {
    /// Input microphone device name
    pub input: Option<String>,
    /// Monitoring output device name (hear yourself)
    pub monitor: Option<String>,
    /// Virtual output device name (for other apps). Auto-detected if omitted.
    pub virtual_device: Option<String>,
    /// Disable noise cancellation (passthrough mode)
    pub no_denoise: bool,
    /// Enable aggressive VAD-based noise gate
    pub hard_mode: bool,
    /// List available audio devices and exit
    pub list_devices: bool,
    /// Path to config file (default: platform config dir)
    pub config: Option<String>,
    /// Run headless (no system tray, useful for scripts)
    pub headless: bool,
    /// Processing mode: light, balanced, or studio
    pub mode: Option<String>,
    /// Deprecated compatibility alias: rnnoise or deepfilter
    pub engine: Option<String>,
    /// Controlled Windows UI regression; opens no audio streams.
    pub ui_smoke_test: bool,
    pub resource_self_test: bool,
    pub open_settings: bool,
}

impl Cli {
    pub fn parse() -> Self {
        let mut pargs = pico_args::Arguments::from_env();

        if pargs.contains(["-h", "--help"]) {
            print_help();
            std::process::exit(0);
        }

        if pargs.contains(["-V", "--version"]) {
            println!("Kynn Voice {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }

        let cli = Cli {
            input: pargs.opt_value_from_str(["-i", "--input"]).unwrap_or(None),
            monitor: pargs
                .opt_value_from_str(["-m", "--monitor"])
                .unwrap_or(None),
            virtual_device: pargs
                .opt_value_from_str(["-v", "--virtual-device"])
                .unwrap_or(None),
            no_denoise: pargs.contains("--no-denoise"),
            hard_mode: pargs.contains("--hard-mode"),
            list_devices: pargs.contains("--list-devices"),
            config: pargs.opt_value_from_str("--config").unwrap_or(None),
            headless: pargs.contains("--headless"),
            mode: pargs.opt_value_from_str("--mode").unwrap_or(None),
            engine: pargs.opt_value_from_str("--engine").unwrap_or(None),
            ui_smoke_test: pargs.contains("--ui-smoke-test"),
            resource_self_test: pargs.contains("--resource-self-test"),
            open_settings: pargs.contains("--settings"),
        };

        let remaining = pargs.finish();
        if !remaining.is_empty() {
            eprintln!("Error: unrecognized arguments: {remaining:?}");
            eprintln!("Run with --help for usage.");
            std::process::exit(1);
        }

        cli
    }
}

fn print_help() {
    println!(
        "\
{name} {version}
{desc}

USAGE:
    {name} [OPTIONS]

OPTIONS:
    -i, --input <NAME>           Input microphone device name
    -m, --monitor <NAME>         Monitoring output device name (hear yourself)
    -v, --virtual-device <NAME>  Virtual output device name (auto-detected if omitted)
        --no-denoise             Disable noise cancellation (passthrough mode)
        --hard-mode              Enable aggressive VAD-based noise gate
        --list-devices           List available audio devices and exit
        --config <PATH>          Path to config file (default: platform config dir)
        --headless               Run headless (no system tray, useful for scripts)
        --settings               Open the settings window at startup
        --ui-smoke-test          Test 50 native windows without opening audio streams
        --resource-self-test    Verify CPU/memory limits in a standalone process
        --mode <MODE>            Processing mode: light (default), balanced, or studio
        --engine <ENGINE>        Deprecated alias: rnnoise or deepfilter
    -h, --help                   Print help
    -V, --version                Print version",
        name = "Kynn Voice",
        version = env!("CARGO_PKG_VERSION"),
        desc = env!("CARGO_PKG_DESCRIPTION"),
    );
}
