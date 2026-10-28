use clap::{Parser, Subcommand};
use margin_api::{AppState, router};
use margin_data::{
    AppConfig, AppStore, ConfigFile, DatasetReadModel, IngestReceipt, NseReportKind, PgStore,
    RuntimeEnv, assemble, connect, ingest_nse_report, migrate, verified_nse_source,
};
use margin_domain::OFFICIAL_CALCULATION_IMPLEMENTED;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{command} is not operational: {detail}")]
    NotOperational {
        command: &'static str,
        detail: &'static str,
    },
    #[error("{detail}")]
    InvalidArgument { detail: String },
    #[error("{detail}")]
    Failed { detail: String },
}

impl EngineError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NotOperational { .. } => 3,
            Self::InvalidArgument { .. } => 2,
            Self::Failed { .. } => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestSource {
    NseClearing,
    Iccl,
}

pub fn parse_ingest_source(source: &str) -> Result<IngestSource, EngineError> {
    match source {
        "NSE" => Ok(IngestSource::NseClearing),
        "ICCL" => Ok(IngestSource::Iccl),
        other => Err(EngineError::InvalidArgument {
            detail: format!("source `{other}` must be NSE or ICCL"),
        }),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "margin-engine",
    version,
    about = "Indian equity derivatives margin engine",
    long_about = "Administrative CLI for the margin engine.\n\nThis build does not replicate NSE Clearing or ICCL margin calculations."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the HTTP server.
    Serve,
    /// Download one verified NSE report and archive the original bytes.
    ///
    /// SPAN risk arrays are not loaded. ICCL stays disabled.
    Ingest {
        /// `NSE` for the verified NSE reports adapter, or `ICCL`.
        #[arg(long)]
        source: String,
        /// Trading date in YYYY-MM-DD.
        #[arg(long)]
        date: String,
        /// NSE report. One of span-begin-of-day, span-intraday-1, span-intraday-2,
        /// span-intraday-3, span-intraday-4, span-end-of-day, exposure-limit, contract-elm.
        #[arg(long)]
        kind: Option<String>,
    },
    /// Re-validate an archived dataset.
    ValidateDataset {
        #[arg(long)]
        dataset_id: String,
    },
    /// List archived risk datasets.
    ListDatasets,
    /// Compare a calculation with a reference result.
    Reconcile {
        #[arg(long)]
        date: String,
    },
    /// Replay a portfolio against the dataset for its original trade date.
    Replay {
        #[arg(long)]
        input: String,
        #[arg(long)]
        date: String,
    },
}

pub fn load_config() -> Result<AppConfig, EngineError> {
    let app = read_config_file("MARGIN_CONFIG", "config/app.toml")?;
    let sources = read_config_file("MARGIN_DATA_SOURCES", "config/data_sources.toml")?;
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(value) if value.trim().is_empty() => {
            return Err(EngineError::InvalidArgument {
                detail: "DATABASE_URL is empty".to_owned(),
            });
        }
        Ok(value) => Some(value),
        Err(_) => None,
    };
    assemble(
        app,
        sources,
        RuntimeEnv {
            database_url,
            bind_addr: std::env::var("BIND_ADDR").ok(),
            log_format: std::env::var("MARGIN_LOG_FORMAT").ok(),
        },
    )
    .map_err(|error| EngineError::Failed {
        detail: error.to_string(),
    })
}

pub async fn open_store(config: &AppConfig) -> Result<AppStore, EngineError> {
    match config.database_url() {
        Some(url) => {
            let pool = connect(url, config.max_db_connections, config.db_acquire_timeout)
                .await
                .map_err(|error| EngineError::Failed {
                    detail: error.to_string(),
                })?;
            migrate(&pool).await.map_err(|error| EngineError::Failed {
                detail: error.to_string(),
            })?;
            Ok(AppStore::Postgres(PgStore::new(pool)))
        }
        None => {
            tracing::warn!("DATABASE_URL is not set; database checks will fail");
            Ok(AppStore::Unavailable {
                reason: "DATABASE_URL is not set".to_owned(),
            })
        }
    }
}

pub async fn execute(
    command: Command,
    config: AppConfig,
    store: AppStore,
) -> Result<(), EngineError> {
    match command {
        Command::Serve => serve(config, store).await,
        Command::Ingest { source, date, kind } => ingest(source, date, kind, &config, &store).await,
        Command::ValidateDataset { dataset_id } => {
            parse_uuid(&dataset_id, "dataset id")?;
            Err(not_operational("validate-dataset"))
        }
        Command::ListDatasets => list_datasets(&store).await,
        Command::Reconcile { date } => {
            let _ = parse_date(&date)?;
            Err(not_operational("reconcile"))
        }
        Command::Replay { input, date } => {
            if input.trim().is_empty() {
                return Err(EngineError::InvalidArgument {
                    detail: "replay input path is empty".to_owned(),
                });
            }
            let _ = parse_date(&date)?;
            Err(not_operational("replay"))
        }
    }
}

async fn ingest(
    source: String,
    date: String,
    kind: Option<String>,
    config: &AppConfig,
    store: &AppStore,
) -> Result<(), EngineError> {
    let source = parse_ingest_source(&source)?;
    let trading_date = parse_date(&date)?;
    let kind = match kind.as_deref() {
        Some(value) => {
            Some(
                NseReportKind::parse(value).map_err(|error| EngineError::InvalidArgument {
                    detail: error.to_string(),
                })?,
            )
        }
        None => None,
    };
    if source == IngestSource::Iccl {
        return Err(EngineError::NotOperational {
            command: "ingest",
            detail: "ICCL risk-parameter download is an HTML form and no file response has been captured",
        });
    }
    let Some(kind) = kind else {
        return Err(EngineError::InvalidArgument {
            detail: format!(
                "NSE ingest requires --kind. Choose one of {}",
                NseReportKind::codes().join(", ")
            ),
        });
    };
    verified_nse_source(config).map_err(|error| EngineError::Failed {
        detail: error.to_string(),
    })?;
    let store = match store {
        AppStore::Postgres(store) => store,
        AppStore::Unavailable { reason } => {
            return Err(EngineError::Failed {
                detail: reason.clone(),
            });
        }
        AppStore::Memory(_) => {
            return Err(EngineError::Failed {
                detail: "ingest requires PostgreSQL".to_owned(),
            });
        }
    };
    let receipt = ingest_nse_report(config, store, kind, trading_date)
        .await
        .map_err(|error| EngineError::Failed {
            detail: error.to_string(),
        })?;
    print_receipt(&receipt);
    if receipt.accepted {
        Ok(())
    } else {
        Err(EngineError::Failed {
            detail: receipt.note,
        })
    }
}

fn print_receipt(receipt: &IngestReceipt) {
    println!("equivalence_claim=none");
    println!("official_calculation_implemented={OFFICIAL_CALCULATION_IMPLEMENTED}");
    println!("disposition={}", receipt.disposition);
    println!("record={}", receipt.record);
    println!("id={}", receipt.id);
    println!("kind={}", receipt.kind);
    println!("filename={}", receipt.filename);
    println!("sha256={}", receipt.sha256);
    if let Some(status) = &receipt.publication_status {
        println!("publication={status}");
    }
    if let Some(status) = &receipt.validation_status {
        println!("validation={status}");
    }
    if let Some(status) = &receipt.check_status {
        println!("check={status}");
    }
    println!("parser={}", receipt.parser_status);
    println!("note={}", receipt.note);
}

async fn list_datasets(store: &AppStore) -> Result<(), EngineError> {
    let page = store
        .list_datasets()
        .await
        .map_err(|error| EngineError::Failed {
            detail: error.to_string(),
        })?;
    println!("equivalence_claim=none");
    println!("official_calculation_implemented={OFFICIAL_CALCULATION_IMPLEMENTED}");
    for dataset in page.datasets {
        let snapshot = match dataset.intraday_sequence {
            Some(sequence) => format!("{}:{sequence}", dataset.snapshot_class.code()),
            None => dataset.snapshot_class.code().to_owned(),
        };
        println!(
            "id={} trading_date={} exchange={} clearing_corporation={} snapshot={snapshot} validation={} publication={} origin={} sha256={}",
            dataset.id,
            dataset.trading_date,
            dataset.exchange.code(),
            dataset.clearing_corporation.code(),
            dataset.validation_status.code(),
            dataset.publication_status,
            dataset.origin.code(),
            dataset.sha256
        );
    }
    if page.truncated {
        println!("truncated=true");
    }
    Ok(())
}

async fn serve(config: AppConfig, store: AppStore) -> Result<(), EngineError> {
    let address = config.bind;
    let state = AppState {
        store,
        body_limit: config.request_body_limit_bytes,
        request_timeout: config.request_timeout,
        max_concurrency: config.max_concurrency,
    };
    let app = router(state);
    let listener =
        tokio::net::TcpListener::bind(address)
            .await
            .map_err(|_| EngineError::Failed {
                detail: format!("could not bind {address}"),
            })?;
    tracing::info!(%address, "http server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|_| EngineError::Failed {
            detail: "http server stopped unexpectedly".to_owned(),
        })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("failed to install the interrupt handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "failed to install the terminate handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

fn read_config_file(env_var: &str, default_path: &str) -> Result<ConfigFile, EngineError> {
    match std::env::var(env_var) {
        Ok(path) => {
            let contents = std::fs::read_to_string(&path).map_err(|_| EngineError::Failed {
                detail: format!("{env_var} could not be read"),
            })?;
            Ok(ConfigFile::Contents(contents))
        }
        Err(_) => match std::fs::read_to_string(default_path) {
            Ok(contents) => Ok(ConfigFile::Contents(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::Absent),
            Err(_) => Err(EngineError::Failed {
                detail: format!("default config {default_path} could not be read"),
            }),
        },
    }
}

fn parse_date(value: &str) -> Result<chrono::NaiveDate, EngineError> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| EngineError::InvalidArgument {
        detail: format!("date `{value}` must use YYYY-MM-DD"),
    })
}

fn parse_uuid(value: &str, what: &str) -> Result<Uuid, EngineError> {
    Uuid::parse_str(value).map_err(|_| EngineError::InvalidArgument {
        detail: format!("{what} must be a UUID"),
    })
}

fn not_operational(command: &'static str) -> EngineError {
    EngineError::NotOperational {
        command,
        detail: "no official file format or margin methodology has been verified",
    }
}
