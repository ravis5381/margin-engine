mod common;

use common::{DatasetParts, at_hour, dataset, sha, trade_date};
use margin_data::AppStore;
use margin_domain::{
    ClearingCorporation, DatasetOrigin, DatasetQuery, DatasetSelector, DomainError, Exchange,
    IngestDisposition, PublicationStatus, SnapshotClass, SnapshotLocator, ValidationStatus,
    classify_ingest, select_dataset,
};
use std::process::Command;

#[test]
fn latest_validated_dataset_never_falls_back() {
    let older = dataset(DatasetParts::published(
        Exchange::Nse,
        ClearingCorporation::Nscc,
        10,
        1,
    ));
    let query = DatasetQuery {
        exchange: Exchange::Nse,
        clearing_corporation: ClearingCorporation::Nscc,
        trade_date: trade_date(),
        snapshot_class: SnapshotClass::EndOfDay,
        selector: DatasetSelector::LatestValidated,
    };
    let candidates = [older.clone()];
    let selected = select_dataset(&candidates, &query).expect("selected");
    assert_eq!(selected.sha256(), older.sha256());

    let mut failed_parts = DatasetParts::published(Exchange::Nse, ClearingCorporation::Nscc, 12, 2);
    failed_parts.validation_status = ValidationStatus::Failed;
    failed_parts.validation_error_summary = Some("synthetic parser rejection".to_owned());
    failed_parts.publication_status = PublicationStatus::Rejected;
    failed_parts.contract_master_version_id = None;
    let failed = dataset(failed_parts);
    assert!(matches!(
        select_dataset(&[older.clone(), failed], &query),
        Err(DomainError::NewerDatasetNotUsable { .. })
    ));

    let mut previous = DatasetParts::published(Exchange::Nse, ClearingCorporation::Nscc, 9, 3);
    previous.trading_date = trade_date().pred_opt().expect("previous day");
    let previous = dataset(previous);
    assert!(matches!(
        select_dataset(&[previous], &query),
        Err(DomainError::StaleRiskData { .. })
    ));
    assert!(matches!(
        select_dataset(&[], &query),
        Err(DomainError::MissingRequiredData { .. })
    ));
}

#[test]
fn equal_rank_with_different_hashes_is_ambiguous() {
    let mut first = DatasetParts::published(Exchange::Bse, ClearingCorporation::Iccl, 11, 4);
    first.snapshot = SnapshotLocator::begin_of_day();
    let mut second = DatasetParts::published(Exchange::Bse, ClearingCorporation::Iccl, 11, 5);
    second.snapshot = SnapshotLocator::begin_of_day();
    let query = DatasetQuery {
        exchange: Exchange::Bse,
        clearing_corporation: ClearingCorporation::Iccl,
        trade_date: trade_date(),
        snapshot_class: SnapshotClass::BeginOfDay,
        selector: DatasetSelector::LatestValidated,
    };
    assert!(matches!(
        select_dataset(&[dataset(first), dataset(second)], &query),
        Err(DomainError::AmbiguousDataset { .. })
    ));
}

#[test]
fn exact_dataset_must_match_the_requested_date() {
    let chosen = dataset(DatasetParts::published(
        Exchange::Bse,
        ClearingCorporation::Iccl,
        8,
        6,
    ));
    let query = DatasetQuery {
        exchange: Exchange::Bse,
        clearing_corporation: ClearingCorporation::Iccl,
        trade_date: trade_date().pred_opt().expect("previous"),
        snapshot_class: SnapshotClass::EndOfDay,
        selector: DatasetSelector::Exact(chosen.id()),
    };
    assert!(matches!(
        select_dataset(&[chosen], &query),
        Err(DomainError::StaleRiskData { .. })
    ));
}

#[test]
fn a_failed_higher_intraday_sequence_blocks_an_older_pass() {
    let mut passed = DatasetParts::published(Exchange::Nse, ClearingCorporation::Nscc, 7, 7);
    passed.snapshot = SnapshotLocator::intraday(1);
    let mut failed = DatasetParts::published(Exchange::Nse, ClearingCorporation::Nscc, 8, 8);
    failed.snapshot = SnapshotLocator::intraday(2);
    failed.validation_status = ValidationStatus::Failed;
    failed.validation_error_summary = Some("sequence 2 failed validation".to_owned());
    failed.publication_status = PublicationStatus::Rejected;
    failed.contract_master_version_id = None;
    let query = DatasetQuery {
        exchange: Exchange::Nse,
        clearing_corporation: ClearingCorporation::Nscc,
        trade_date: trade_date(),
        snapshot_class: SnapshotClass::Intraday,
        selector: DatasetSelector::LatestValidated,
    };
    assert!(matches!(
        select_dataset(&[dataset(passed), dataset(failed)], &query),
        Err(DomainError::NewerDatasetNotUsable { .. })
    ));
}

#[test]
fn duplicate_bytes_do_not_create_another_version() {
    let existing = dataset(DatasetParts::published(
        Exchange::Nse,
        ClearingCorporation::Nscc,
        6,
        9,
    ));
    assert!(matches!(
        classify_ingest(std::slice::from_ref(&existing), existing.sha256()),
        IngestDisposition::Duplicate { .. }
    ));
    assert!(matches!(
        classify_ingest(&[existing], &sha(10)),
        IngestDisposition::New
    ));
}

#[test]
fn official_origin_requires_a_host_and_synthetic_cannot_be_official() {
    let mut official = DatasetParts::published(Exchange::Nse, ClearingCorporation::Nscc, 5, 11);
    official.origin = DatasetOrigin::Official;
    official.source_host = Some("www.nseclearing.in".to_owned());
    let official = dataset(official);
    assert!(official.require_official().is_ok());
    assert_eq!(at_hour(5), official.download_timestamp());
}

#[test]
fn cli_ingest_needs_a_database_for_iccl_and_nse() {
    let output = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .args(["ingest", "--source", "ICCL", "--date", "2026-10-28"])
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .env_remove("DATABASE_URL")
        .env_remove("MARGIN_CONFIG")
        .env_remove("MARGIN_DATA_SOURCES")
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr");
    assert!(stderr.contains("DATABASE_URL is not set"), "{stderr}");

    let invalid = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .args(["ingest", "--source", "FOO", "--date", "2026-10-28"])
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .env_remove("DATABASE_URL")
        .env_remove("MARGIN_CONFIG")
        .env_remove("MARGIN_DATA_SOURCES")
        .output()
        .expect("run");
    assert_eq!(invalid.status.code(), Some(2));

    let help = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .arg("--help")
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .output()
        .expect("help");
    assert!(help.status.success());
    let stdout = String::from_utf8(help.stdout).expect("stdout");
    assert!(stdout.contains("ingest"));
    assert!(stdout.contains("does not replicate"));

    let listed = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .arg("list-datasets")
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .env_remove("DATABASE_URL")
        .env_remove("MARGIN_CONFIG")
        .env_remove("MARGIN_DATA_SOURCES")
        .output()
        .expect("list");
    assert_eq!(listed.status.code(), Some(1));

    let missing_kind = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .args(["ingest", "--source", "NSE", "--date", "2026-10-27"])
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .env_remove("DATABASE_URL")
        .env_remove("MARGIN_CONFIG")
        .env_remove("MARGIN_DATA_SOURCES")
        .output()
        .expect("missing kind");
    assert_eq!(missing_kind.status.code(), Some(2));

    let sources = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/config/data_sources.example.toml"
    );
    let needs_database = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .args([
            "ingest",
            "--source",
            "NSE",
            "--date",
            "2026-10-27",
            "--kind",
            "span-end-of-day",
        ])
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .env("MARGIN_DATA_SOURCES", sources)
        .env_remove("DATABASE_URL")
        .env_remove("MARGIN_CONFIG")
        .output()
        .expect("needs database");
    assert_eq!(needs_database.status.code(), Some(1));
    let stderr = String::from_utf8(needs_database.stderr).expect("stderr");
    assert!(stderr.contains("DATABASE_URL is not set"));
}

#[test]
fn reconcile_rejects_a_bad_reference_before_it_quotes() {
    let missing = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .args(["reconcile", "--reference", "missing-reference.json"])
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .env_remove("DATABASE_URL")
        .env_remove("MARGIN_CONFIG")
        .env_remove("MARGIN_DATA_SOURCES")
        .output()
        .expect("missing reference");
    assert_eq!(missing.status.code(), Some(2));
    let stderr = String::from_utf8(missing.stderr).expect("stderr");
    assert!(stderr.contains("could not be read"), "{stderr}");

    let path = std::env::temp_dir().join("margin-engine-reference.json");
    std::fs::write(
        &path,
        r#"{"source":"operator","exchange":"NSE","trading_datetime":"yesterday"}"#,
    )
    .expect("write");
    let invalid = Command::new(env!("CARGO_BIN_EXE_margin-engine"))
        .args(["reconcile", "--reference", path.to_str().expect("path")])
        .env("MARGIN_ENGINE_SKIP_DOTENV", "1")
        .env_remove("DATABASE_URL")
        .env_remove("MARGIN_CONFIG")
        .env_remove("MARGIN_DATA_SOURCES")
        .output()
        .expect("invalid reference");
    assert_eq!(invalid.status.code(), Some(2));
    let stderr = String::from_utf8(invalid.stderr).expect("stderr");
    assert!(stderr.contains("reference JSON"), "{stderr}");
}

#[tokio::test]
async fn migrations_preserve_bytes_and_reject_published_rewrites() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL is unset; skipping migration integration test");
        return;
    };
    if url.trim().is_empty() {
        eprintln!("DATABASE_URL is empty; skipping migration integration test");
        return;
    }
    let pool = margin_data::connect(&url, 2, std::time::Duration::from_secs(5))
        .await
        .expect("connect");
    margin_data::migrate(&pool).await.expect("migrate");

    let master_id = uuid::Uuid::now_v7();
    let file_id = uuid::Uuid::now_v7();
    let dataset_id = uuid::Uuid::now_v7();
    let digest = sha(21);
    let version = format!("synthetic-{master_id}");
    let content = b"synthetic-not-an-official-risk-file";

    sqlx::query(
        "INSERT INTO contract_master_versions \
         (id, exchange, clearing_corporation, version_label, effective_date, sha256, source_filename) \
         VALUES ($1, 'NSE', 'NSCCL', $2, '2026-10-28', $3, 'master.txt')",
    )
    .bind(master_id)
    .bind(&version)
    .bind(digest.as_str())
    .execute(&pool)
    .await
    .expect("master");

    sqlx::query(
        "INSERT INTO risk_files (id, original_filename, sha256, byte_size, content) \
         VALUES ($1, 'synthetic.dat', $2, $3, $4)",
    )
    .bind(file_id)
    .bind(digest.as_str())
    .bind(i64::try_from(content.len()).expect("len"))
    .bind(content.as_slice())
    .execute(&pool)
    .await
    .expect("file");

    sqlx::query(
        "INSERT INTO risk_datasets ( \
            id, exchange, clearing_corporation, trading_date, snapshot_class, \
            publication_timestamp, download_timestamp, original_filename, source_host, sha256, \
            file_format, parser_version, contract_master_version_id, validation_status, \
            publication_status, origin, risk_file_id \
         ) VALUES ( \
            $1, 'NSE', 'NSCCL', '2026-10-28', 'end_of_day', \
            $2, $2, 'synthetic.dat', NULL, $3, \
            'synthetic', 'none', $4, 'passed', \
            'published', 'synthetic', $5 \
         )",
    )
    .bind(dataset_id)
    .bind(at_hour(4))
    .bind(digest.as_str())
    .bind(master_id)
    .bind(file_id)
    .execute(&pool)
    .await
    .expect("dataset");

    let store = AppStore::Postgres(margin_data::PgStore::new(pool.clone()));
    let loaded = margin_data::DatasetReadModel::get_dataset(
        &store,
        margin_domain::DatasetId::from_uuid(dataset_id),
    )
    .await
    .expect("read")
    .expect("present");
    assert_eq!(loaded.sha256().as_str(), digest.as_str());
    assert!(loaded.require_official().is_err());

    let rewritten = sqlx::query("UPDATE risk_datasets SET sha256 = $2 WHERE id = $1")
        .bind(dataset_id)
        .bind("ab".repeat(32))
        .execute(&pool)
        .await;
    assert!(rewritten.is_err());

    let mutated = sqlx::query("UPDATE risk_files SET content = $2 WHERE id = $1")
        .bind(file_id)
        .bind(b"changed".as_slice())
        .execute(&pool)
        .await;
    assert!(mutated.is_err());

    let other_file = uuid::Uuid::now_v7();
    let other_digest = sha(22);
    sqlx::query(
        "INSERT INTO risk_files (id, original_filename, sha256, byte_size, content) \
         VALUES ($1, 'other.dat', $2, $3, $4)",
    )
    .bind(other_file)
    .bind(other_digest.as_str())
    .bind(4_i64)
    .bind(b"nope".as_slice())
    .execute(&pool)
    .await
    .expect("second file");
    let invalid = sqlx::query(
        "INSERT INTO risk_datasets ( \
            id, exchange, clearing_corporation, trading_date, snapshot_class, \
            download_timestamp, original_filename, sha256, file_format, parser_version, \
            validation_status, publication_status, origin, risk_file_id \
         ) VALUES ( \
            $1, 'NSE', 'NSCCL', '2026-10-28', 'end_of_day', \
            $2, 'other.dat', $3, 'synthetic', 'none', \
            'failed', 'published', 'synthetic', $4 \
         )",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(at_hour(3))
    .bind(other_digest.as_str())
    .bind(other_file)
    .execute(&pool)
    .await;
    assert!(invalid.is_err());
}
