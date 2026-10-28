-- Original bytes for verified NSE files that are not SPAN risk-parameter snapshots.
-- Percentages in these files are not margin rules. Rows cannot be rewritten.

CREATE TABLE official_auxiliary_files (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL,
    exchange TEXT NOT NULL,
    clearing_corporation TEXT NOT NULL,
    trading_date DATE NOT NULL,
    original_filename TEXT NOT NULL,
    source_host TEXT NOT NULL,
    sha256 TEXT NOT NULL UNIQUE,
    byte_size BIGINT NOT NULL,
    content BYTEA NOT NULL,
    download_timestamp TIMESTAMPTZ NOT NULL,
    check_status TEXT NOT NULL,
    check_summary TEXT,
    parser_status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT official_auxiliary_files_kind_chk CHECK (
        kind IN ('exposure_limit_csv', 'contract_elm_gzip')
    ),
    CONSTRAINT official_auxiliary_files_exchange_chk CHECK (exchange IN ('NSE', 'BSE')),
    CONSTRAINT official_auxiliary_files_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT official_auxiliary_files_sha_chk CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT official_auxiliary_files_size_chk CHECK (
        byte_size >= 0 AND byte_size = octet_length(content)
    ),
    CONSTRAINT official_auxiliary_files_filename_chk CHECK (
        original_filename <> ''
        AND position('/' IN original_filename) = 0
        AND position(chr(92) IN original_filename) = 0
        AND position('..' IN original_filename) = 0
    ),
    CONSTRAINT official_auxiliary_files_host_chk CHECK (
        length(trim(source_host)) > 0 AND position('/' IN source_host) = 0
    ),
    CONSTRAINT official_auxiliary_files_check_chk CHECK (
        (
            check_status = 'container_checked'
            AND check_summary IS NULL
        )
        OR (
            check_status = 'rejected'
            AND check_summary IS NOT NULL
            AND length(trim(check_summary)) > 0
        )
    ),
    CONSTRAINT official_auxiliary_files_parser_chk CHECK (parser_status = 'not_implemented')
);

CREATE INDEX official_auxiliary_files_lookup_idx
    ON official_auxiliary_files (trading_date, clearing_corporation, exchange, kind);

CREATE TRIGGER official_auxiliary_files_no_update
    BEFORE UPDATE ON official_auxiliary_files
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER official_auxiliary_files_no_delete
    BEFORE DELETE ON official_auxiliary_files
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

ALTER TABLE risk_ingestion_runs
    ADD COLUMN auxiliary_file_id UUID REFERENCES official_auxiliary_files (id);

ALTER TABLE risk_ingestion_runs
    ADD CONSTRAINT risk_ingestion_runs_one_target_chk CHECK (
        dataset_id IS NULL OR auxiliary_file_id IS NULL
    );
