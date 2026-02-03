use clap::Parser;
use pmm_sim::{App, CliArgs, PMMCfg};
use tracing::{debug, info};
use tracing_subscriber::{EnvFilter, fmt::time::UtcTime};

fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_thread_ids(true)
        .with_line_number(true)
        .with_target(true)
        .with_timer(UtcTime::rfc_3339())
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::default().add_directive(tracing::Level::INFO.into())),
        )
        .init();

    let args = CliArgs::parse();
    let cfg = PMMCfg::load(args.command.setup_path())?;

    info!(command = args.command.name(), ?args);
    debug!(?cfg);

    App::new(args, cfg).start()
}
