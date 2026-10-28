use margin_data::LogFormat;
use tracing_subscriber::EnvFilter;

pub fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let result = match format {
        LogFormat::Json => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .try_init(),
        LogFormat::Text => tracing_subscriber::fmt().with_env_filter(filter).try_init(),
    };
    if let Err(error) = result {
        eprintln!("logging could not be initialized: {error}");
    }
}
