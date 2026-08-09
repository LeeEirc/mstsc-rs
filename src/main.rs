use std::process::ExitCode;

use mstsc_rs::cli::Cli;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mstsc_rs=info".into()),
        )
        .without_time()
        .init();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mstsc-rs: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> mstsc_rs::Result<()> {
    let cli = Cli::parse_compatible(std::env::args_os());
    let (config, dry_run) = cli.into_session_config()?;

    if dry_run {
        print!("{}", config.rdp_settings_text());
        return Ok(());
    }

    mstsc_rs::windows::run(config)
}
