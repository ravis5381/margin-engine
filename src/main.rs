use clap::Parser;
use margin_engine::cli::{Cli, EngineError, load_config, open_store};
use margin_engine::logging;

#[tokio::main]
async fn main() {
    let exit_code = match run().await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("margin-engine: {error}");
            error.exit_code()
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

async fn run() -> Result<(), EngineError> {
    if std::env::var_os("MARGIN_ENGINE_SKIP_DOTENV").is_none() {
        let _ = dotenvy::dotenv();
    }
    let cli = Cli::parse();
    let config = load_config()?;
    logging::init(config.log_format);
    let store = open_store(&config).await?;
    margin_engine::cli::execute(cli.command, config, store).await
}
