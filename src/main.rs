use clap::Parser;
use tracing_log::LogTracer;
use vusbipd_ccid::{
    app::Application,
    config::{AppConfig, Cli},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let config = AppConfig::try_from(cli)?;
    Application::new(config).run().await
}

fn init_tracing() {
    let _ = LogTracer::init();
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,usbip=info,vusbipd_ccid=debug".into());

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}
