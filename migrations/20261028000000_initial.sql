-- Initial schema for immutable risk data, contracts, calculations, and reconciliation.
-- Monetary amounts use NUMERIC. No floating-point columns are used for money.

CREATE FUNCTION reject_row_mutation() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION '% on % is not permitted', TG_OP, TG_TABLE_NAME;
END;
$$;

CREATE FUNCTION protect_risk_datasets() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.publication_status = 'published'
       AND NEW.publication_status = 'superseded'
       AND NEW.superseded_by IS NOT NULL
       AND NEW.id = OLD.id
       AND NEW.exchange = OLD.exchange
       AND NEW.clearing_corporation = OLD.clearing_corporation
       AND NEW.trading_date = OLD.trading_date
       AND NEW.snapshot_class = OLD.snapshot_class
       AND NEW.intraday_sequence IS NOT DISTINCT FROM OLD.intraday_sequence
       AND NEW.publication_timestamp IS NOT DISTINCT FROM OLD.publication_timestamp
       AND NEW.download_timestamp = OLD.download_timestamp
       AND NEW.original_filename = OLD.original_filename
       AND NEW.source_host IS NOT DISTINCT FROM OLD.source_host
       AND NEW.sha256 = OLD.sha256
       AND NEW.file_format = OLD.file_format
       AND NEW.parser_version = OLD.parser_version
       AND NEW.contract_master_version_id IS NOT DISTINCT FROM OLD.contract_master_version_id
       AND NEW.validation_status = OLD.validation_status
       AND NEW.validation_error_summary IS NOT DISTINCT FROM OLD.validation_error_summary
       AND NEW.origin = OLD.origin
       AND NEW.risk_file_id = OLD.risk_file_id
    THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'risk dataset rows are immutable except published -> superseded';
END;
$$;

CREATE FUNCTION protect_reconciliation_differences() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.id <> OLD.id
       OR NEW.run_id <> OLD.run_id
       OR NEW.component_code <> OLD.component_code
       OR NEW.expected_amount <> OLD.expected_amount
       OR NEW.calculated_amount <> OLD.calculated_amount
       OR NEW.absolute_difference <> OLD.absolute_difference
       OR NEW.relative_difference IS DISTINCT FROM OLD.relative_difference
       OR NEW.currency <> OLD.currency
       OR NEW.material <> OLD.material
       OR NEW.within_diagnostic_tolerance <> OLD.within_diagnostic_tolerance
       OR NEW.diagnostic_category IS DISTINCT FROM OLD.diagnostic_category
       OR NEW.calculation_trace <> OLD.calculation_trace
       OR NEW.risk_dataset_id IS DISTINCT FROM OLD.risk_dataset_id
       OR NEW.rule_version_id IS DISTINCT FROM OLD.rule_version_id
       OR NEW.calculation_id IS DISTINCT FROM OLD.calculation_id
       OR NEW.position_refs <> OLD.position_refs
    THEN
        RAISE EXCEPTION 'reconciliation difference amounts are immutable';
    END IF;
    IF NEW.resolution_status = OLD.resolution_status
       AND NEW.resolution_note IS NOT DISTINCT FROM OLD.resolution_note
    THEN
        RETURN NEW;
    END IF;
    IF OLD.resolution_status <> 'unresolved' THEN
        RAISE EXCEPTION 'reconciliation resolution cannot be rewritten';
    END IF;
    IF NEW.resolution_status NOT IN ('explained', 'accepted_exception') THEN
        RAISE EXCEPTION 'invalid resolution transition';
    END IF;
    IF NEW.resolution_note IS NULL OR length(trim(NEW.resolution_note)) = 0 THEN
        RAISE EXCEPTION 'resolution requires a note';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TABLE risk_files (
    id UUID PRIMARY KEY,
    original_filename TEXT NOT NULL,
    sha256 TEXT NOT NULL UNIQUE,
    byte_size BIGINT NOT NULL,
    content BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT risk_files_sha_chk CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT risk_files_filename_chk CHECK (
        original_filename <> ''
        AND position('/' IN original_filename) = 0
        AND position(chr(92) IN original_filename) = 0
        AND position('..' IN original_filename) = 0
    ),
    CONSTRAINT risk_files_size_chk CHECK (byte_size >= 0 AND byte_size = octet_length(content))
);

CREATE TRIGGER risk_files_no_update
    BEFORE UPDATE ON risk_files
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER risk_files_no_delete
    BEFORE DELETE ON risk_files
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE contract_master_versions (
    id UUID PRIMARY KEY,
    exchange TEXT NOT NULL,
    clearing_corporation TEXT NOT NULL,
    version_label TEXT NOT NULL,
    effective_date DATE NOT NULL,
    sha256 TEXT NOT NULL,
    source_filename TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT contract_master_versions_exchange_chk CHECK (exchange IN ('NSE', 'BSE')),
    CONSTRAINT contract_master_versions_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT contract_master_versions_sha_chk CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    UNIQUE (clearing_corporation, exchange, version_label)
);

CREATE TRIGGER contract_master_versions_no_update
    BEFORE UPDATE ON contract_master_versions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER contract_master_versions_no_delete
    BEFORE DELETE ON contract_master_versions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE risk_datasets (
    id UUID PRIMARY KEY,
    exchange TEXT NOT NULL,
    clearing_corporation TEXT NOT NULL,
    trading_date DATE NOT NULL,
    snapshot_class TEXT NOT NULL,
    intraday_sequence INTEGER,
    publication_timestamp TIMESTAMPTZ,
    download_timestamp TIMESTAMPTZ NOT NULL,
    original_filename TEXT NOT NULL,
    source_host TEXT,
    sha256 TEXT NOT NULL UNIQUE,
    file_format TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    contract_master_version_id UUID REFERENCES contract_master_versions (id),
    validation_status TEXT NOT NULL,
    validation_error_summary TEXT,
    publication_status TEXT NOT NULL,
    superseded_by UUID,
    origin TEXT NOT NULL,
    risk_file_id UUID NOT NULL UNIQUE REFERENCES risk_files (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT risk_datasets_exchange_chk CHECK (exchange IN ('NSE', 'BSE')),
    CONSTRAINT risk_datasets_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT risk_datasets_snapshot_chk CHECK (snapshot_class IN ('begin_of_day', 'intraday', 'end_of_day')),
    CONSTRAINT risk_datasets_sequence_chk CHECK (
        (snapshot_class = 'intraday' AND intraday_sequence IS NOT NULL AND intraday_sequence >= 0)
        OR (snapshot_class <> 'intraday' AND intraday_sequence IS NULL)
    ),
    CONSTRAINT risk_datasets_sha_chk CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT risk_datasets_validation_chk CHECK (validation_status IN ('pending', 'passed', 'failed')),
    CONSTRAINT risk_datasets_publication_chk CHECK (publication_status IN ('draft', 'published', 'superseded', 'rejected')),
    CONSTRAINT risk_datasets_origin_chk CHECK (origin IN ('synthetic', 'official')),
    CONSTRAINT risk_datasets_origin_host_chk CHECK (
        (origin = 'synthetic' AND source_host IS NULL)
        OR (origin = 'official' AND source_host IS NOT NULL AND length(trim(source_host)) > 0)
    ),
    CONSTRAINT risk_datasets_filename_chk CHECK (
        original_filename <> ''
        AND position('/' IN original_filename) = 0
        AND position(chr(92) IN original_filename) = 0
        AND position('..' IN original_filename) = 0
    ),
    CONSTRAINT risk_datasets_lifecycle_chk CHECK (
        (
            publication_status = 'draft'
            AND validation_status = 'pending'
            AND validation_error_summary IS NULL
            AND superseded_by IS NULL
        )
        OR (
            publication_status = 'rejected'
            AND validation_status = 'failed'
            AND validation_error_summary IS NOT NULL
            AND length(trim(validation_error_summary)) > 0
            AND superseded_by IS NULL
        )
        OR (
            publication_status = 'published'
            AND validation_status = 'passed'
            AND validation_error_summary IS NULL
            AND contract_master_version_id IS NOT NULL
            AND superseded_by IS NULL
        )
        OR (
            publication_status = 'superseded'
            AND validation_status = 'passed'
            AND validation_error_summary IS NULL
            AND contract_master_version_id IS NOT NULL
            AND superseded_by IS NOT NULL
        )
    )
);

ALTER TABLE risk_datasets
    ADD CONSTRAINT risk_datasets_superseded_by_fk
    FOREIGN KEY (superseded_by) REFERENCES risk_datasets (id);

CREATE UNIQUE INDEX risk_datasets_snapshot_hash_idx
    ON risk_datasets (
        clearing_corporation,
        exchange,
        trading_date,
        snapshot_class,
        COALESCE(intraday_sequence, -1),
        sha256
    );

CREATE INDEX risk_datasets_lookup_idx
    ON risk_datasets (trading_date, clearing_corporation, exchange, publication_status, validation_status);

CREATE TRIGGER risk_datasets_protect_update
    BEFORE UPDATE ON risk_datasets
    FOR EACH ROW
    EXECUTE FUNCTION protect_risk_datasets();

CREATE TRIGGER risk_datasets_no_delete
    BEFORE DELETE ON risk_datasets
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE contracts (
    id TEXT PRIMARY KEY,
    contract_master_version_id UUID NOT NULL REFERENCES contract_master_versions (id),
    exchange TEXT NOT NULL,
    clearing_corporation TEXT NOT NULL,
    underlying TEXT NOT NULL,
    instrument_type TEXT NOT NULL,
    expiry_date DATE NOT NULL,
    strike_amount NUMERIC,
    strike_currency TEXT,
    option_type TEXT,
    lot_size INTEGER NOT NULL,
    multiplier NUMERIC NOT NULL,
    currency TEXT NOT NULL,
    source_contract_id TEXT NOT NULL,
    effective_trading_date DATE NOT NULL,
    CONSTRAINT contracts_exchange_chk CHECK (exchange IN ('NSE', 'BSE')),
    CONSTRAINT contracts_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT contracts_instrument_chk CHECK (
        instrument_type IN ('INDEX_FUTURE', 'INDEX_OPTION', 'STOCK_FUTURE', 'STOCK_OPTION')
    ),
    CONSTRAINT contracts_option_type_chk CHECK (option_type IS NULL OR option_type IN ('CALL', 'PUT')),
    CONSTRAINT contracts_lot_chk CHECK (lot_size > 0),
    CONSTRAINT contracts_multiplier_chk CHECK (multiplier > 0),
    CONSTRAINT contracts_currency_chk CHECK (currency = 'INR'),
    CONSTRAINT contracts_strike_currency_chk CHECK (strike_currency IS NULL OR strike_currency = 'INR'),
    CONSTRAINT contracts_shape_chk CHECK (
        (
            instrument_type IN ('INDEX_OPTION', 'STOCK_OPTION')
            AND strike_amount IS NOT NULL
            AND strike_amount >= 0
            AND strike_currency = 'INR'
            AND option_type IS NOT NULL
        )
        OR (
            instrument_type IN ('INDEX_FUTURE', 'STOCK_FUTURE')
            AND strike_amount IS NULL
            AND strike_currency IS NULL
            AND option_type IS NULL
        )
    ),
    UNIQUE (contract_master_version_id, source_contract_id, exchange, clearing_corporation)
);

CREATE INDEX contracts_lookup_idx
    ON contracts (exchange, clearing_corporation, underlying, expiry_date, instrument_type);

CREATE TRIGGER contracts_no_update
    BEFORE UPDATE ON contracts
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER contracts_no_delete
    BEFORE DELETE ON contracts
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE scenario_definitions (
    id UUID PRIMARY KEY,
    dataset_id UUID NOT NULL REFERENCES risk_datasets (id),
    scenario_index INTEGER NOT NULL,
    scenario_code TEXT NOT NULL,
    pnl_convention TEXT NOT NULL,
    sign_convention TEXT NOT NULL,
    CONSTRAINT scenario_definitions_index_chk CHECK (scenario_index >= 0),
    CONSTRAINT scenario_definitions_convention_chk CHECK (pnl_convention IN ('profit_and_loss', 'loss')),
    UNIQUE (dataset_id, scenario_index),
    UNIQUE (dataset_id, scenario_code)
);

CREATE TRIGGER scenario_definitions_no_update
    BEFORE UPDATE ON scenario_definitions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER scenario_definitions_no_delete
    BEFORE DELETE ON scenario_definitions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE risk_arrays (
    id UUID PRIMARY KEY,
    dataset_id UUID NOT NULL REFERENCES risk_datasets (id),
    contract_id TEXT NOT NULL REFERENCES contracts (id),
    quantity_basis TEXT NOT NULL,
    currency TEXT NOT NULL,
    parser_diagnostics TEXT,
    CONSTRAINT risk_arrays_basis_chk CHECK (quantity_basis IN ('lot', 'contract', 'underlying_unit')),
    CONSTRAINT risk_arrays_currency_chk CHECK (currency = 'INR'),
    UNIQUE (dataset_id, contract_id)
);

CREATE INDEX risk_arrays_contract_idx ON risk_arrays (contract_id);

CREATE TRIGGER risk_arrays_no_update
    BEFORE UPDATE ON risk_arrays
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER risk_arrays_no_delete
    BEFORE DELETE ON risk_arrays
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE risk_array_values (
    risk_array_id UUID NOT NULL REFERENCES risk_arrays (id),
    scenario_index INTEGER NOT NULL,
    scenario_code TEXT NOT NULL,
    amount NUMERIC NOT NULL,
    currency TEXT NOT NULL,
    pnl_convention TEXT NOT NULL,
    quantity_basis TEXT NOT NULL,
    scale_factor NUMERIC NOT NULL,
    source_raw TEXT NOT NULL,
    source_locator TEXT,
    PRIMARY KEY (risk_array_id, scenario_index),
    CONSTRAINT risk_array_values_index_chk CHECK (scenario_index >= 0),
    CONSTRAINT risk_array_values_currency_chk CHECK (currency = 'INR'),
    CONSTRAINT risk_array_values_convention_chk CHECK (pnl_convention IN ('profit_and_loss', 'loss')),
    CONSTRAINT risk_array_values_basis_chk CHECK (quantity_basis IN ('lot', 'contract', 'underlying_unit')),
    CONSTRAINT risk_array_values_scale_chk CHECK (scale_factor > 0),
    CONSTRAINT risk_array_values_source_chk CHECK (length(trim(source_raw)) > 0)
);

COMMENT ON TABLE risk_array_values IS
    'Typed scenario values with the original source token. Automatic deletion is not implemented.';

CREATE TRIGGER risk_array_values_no_update
    BEFORE UPDATE ON risk_array_values
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER risk_array_values_no_delete
    BEFORE DELETE ON risk_array_values
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE margin_rule_versions (
    id TEXT PRIMARY KEY,
    clearing_corporation TEXT NOT NULL,
    exchange TEXT,
    product_scope TEXT NOT NULL,
    effective_from DATE NOT NULL,
    effective_to DATE,
    source_reference TEXT NOT NULL,
    description TEXT NOT NULL,
    CONSTRAINT margin_rule_versions_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT margin_rule_versions_exchange_chk CHECK (exchange IS NULL OR exchange IN ('NSE', 'BSE')),
    CONSTRAINT margin_rule_versions_reference_chk CHECK (length(trim(source_reference)) > 0),
    CONSTRAINT margin_rule_versions_dates_chk CHECK (effective_to IS NULL OR effective_to >= effective_from)
);

CREATE INDEX margin_rule_versions_lookup_idx
    ON margin_rule_versions (clearing_corporation, effective_from);

CREATE TRIGGER margin_rule_versions_no_update
    BEFORE UPDATE ON margin_rule_versions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER margin_rule_versions_no_delete
    BEFORE DELETE ON margin_rule_versions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE portfolio_snapshots (
    id UUID PRIMARY KEY,
    portfolio_id TEXT NOT NULL,
    aggregation_scope_kind TEXT NOT NULL,
    aggregation_scope_id TEXT NOT NULL,
    aggregation_rule_reference TEXT,
    exchange TEXT NOT NULL,
    clearing_corporation TEXT NOT NULL,
    trading_date DATE NOT NULL,
    source_system TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT portfolio_snapshots_exchange_chk CHECK (exchange IN ('NSE', 'BSE')),
    CONSTRAINT portfolio_snapshots_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT portfolio_snapshots_scope_chk CHECK (
        aggregation_scope_kind IN ('client_account', 'portfolio', 'permitted_group')
    )
);

CREATE INDEX portfolio_snapshots_lookup_idx
    ON portfolio_snapshots (portfolio_id, trading_date, clearing_corporation);

CREATE TRIGGER portfolio_snapshots_no_update
    BEFORE UPDATE ON portfolio_snapshots
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER portfolio_snapshots_no_delete
    BEFORE DELETE ON portfolio_snapshots
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE portfolio_positions (
    id UUID PRIMARY KEY,
    snapshot_id UUID NOT NULL REFERENCES portfolio_snapshots (id),
    contract_id TEXT NOT NULL REFERENCES contracts (id),
    quantity NUMERIC NOT NULL,
    quantity_unit TEXT NOT NULL,
    average_trade_price NUMERIC,
    valuation_price NUMERIC,
    valuation_source TEXT,
    valuation_as_of TIMESTAMPTZ,
    origin TEXT NOT NULL,
    position_timestamp TIMESTAMPTZ NOT NULL,
    CONSTRAINT portfolio_positions_unit_chk CHECK (quantity_unit IN ('lot', 'contract', 'underlying_unit')),
    CONSTRAINT portfolio_positions_valuation_chk CHECK (
        (valuation_price IS NULL AND valuation_source IS NULL AND valuation_as_of IS NULL)
        OR (valuation_price IS NOT NULL AND valuation_source IS NOT NULL AND valuation_as_of IS NOT NULL)
    )
);

CREATE INDEX portfolio_positions_snapshot_idx ON portfolio_positions (snapshot_id);
CREATE INDEX portfolio_positions_contract_idx ON portfolio_positions (contract_id);

CREATE TRIGGER portfolio_positions_no_update
    BEFORE UPDATE ON portfolio_positions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER portfolio_positions_no_delete
    BEFORE DELETE ON portfolio_positions
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE risk_ingestion_runs (
    id UUID PRIMARY KEY,
    source_id TEXT NOT NULL,
    exchange TEXT NOT NULL,
    clearing_corporation TEXT NOT NULL,
    trading_date DATE NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    status TEXT NOT NULL,
    dataset_id UUID REFERENCES risk_datasets (id),
    error_summary TEXT,
    CONSTRAINT risk_ingestion_runs_exchange_chk CHECK (exchange IN ('NSE', 'BSE')),
    CONSTRAINT risk_ingestion_runs_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT risk_ingestion_runs_status_chk CHECK (status IN ('running', 'succeeded', 'failed', 'rejected'))
);

CREATE INDEX risk_ingestion_runs_date_idx
    ON risk_ingestion_runs (trading_date, clearing_corporation, status);

CREATE TABLE margin_calculations (
    id UUID PRIMARY KEY,
    portfolio_snapshot_id UUID NOT NULL REFERENCES portfolio_snapshots (id),
    trade_date DATE NOT NULL,
    clearing_corporation TEXT NOT NULL,
    exchange TEXT NOT NULL,
    risk_dataset_id UUID NOT NULL REFERENCES risk_datasets (id),
    margin_rule_version_id TEXT REFERENCES margin_rule_versions (id),
    calculation_version TEXT NOT NULL,
    status TEXT NOT NULL,
    currency TEXT NOT NULL,
    inputs_sha256 TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT margin_calculations_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT margin_calculations_exchange_chk CHECK (exchange IN ('NSE', 'BSE')),
    CONSTRAINT margin_calculations_status_chk CHECK (status IN ('indicative', 'rejected')),
    CONSTRAINT margin_calculations_currency_chk CHECK (currency = 'INR'),
    CONSTRAINT margin_calculations_sha_chk CHECK (inputs_sha256 ~ '^[0-9a-f]{64}$')
);

CREATE INDEX margin_calculations_lookup_idx
    ON margin_calculations (portfolio_snapshot_id, trade_date, clearing_corporation);

CREATE TRIGGER margin_calculations_no_update
    BEFORE UPDATE ON margin_calculations
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER margin_calculations_no_delete
    BEFORE DELETE ON margin_calculations
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE margin_components (
    calculation_id UUID NOT NULL REFERENCES margin_calculations (id),
    component_code TEXT NOT NULL,
    amount NUMERIC NOT NULL,
    currency TEXT NOT NULL,
    rule_reference TEXT NOT NULL,
    PRIMARY KEY (calculation_id, component_code),
    CONSTRAINT margin_components_currency_chk CHECK (currency = 'INR'),
    CONSTRAINT margin_components_reference_chk CHECK (length(trim(rule_reference)) > 0),
    CONSTRAINT margin_components_code_chk CHECK (component_code IN (
        'scanning_risk',
        'spread_charge',
        'short_option_minimum',
        'other_span_adjustments',
        'net_option_value',
        'span_margin',
        'elm',
        'delivery_margin',
        'premium_obligation',
        'crystallised_obligations',
        'additional_margin',
        'cross_margin_benefit',
        'total_margin'
    ))
);

CREATE TRIGGER margin_components_no_update
    BEFORE UPDATE ON margin_components
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER margin_components_no_delete
    BEFORE DELETE ON margin_components
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE reconciliation_runs (
    id UUID PRIMARY KEY,
    trade_date DATE NOT NULL,
    clearing_corporation TEXT NOT NULL,
    portfolio_snapshot_id UUID REFERENCES portfolio_snapshots (id),
    calculation_id UUID REFERENCES margin_calculations (id),
    risk_dataset_id UUID REFERENCES risk_datasets (id),
    reference_source TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT reconciliation_runs_cc_chk CHECK (clearing_corporation IN ('NSCCL', 'ICCL')),
    CONSTRAINT reconciliation_runs_status_chk CHECK (status IN ('running', 'completed', 'failed'))
);

CREATE INDEX reconciliation_runs_lookup_idx
    ON reconciliation_runs (trade_date, clearing_corporation);

CREATE TABLE reconciliation_differences (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES reconciliation_runs (id),
    component_code TEXT NOT NULL,
    expected_amount NUMERIC NOT NULL,
    calculated_amount NUMERIC NOT NULL,
    absolute_difference NUMERIC NOT NULL,
    relative_difference NUMERIC,
    currency TEXT NOT NULL,
    material BOOLEAN NOT NULL,
    within_diagnostic_tolerance BOOLEAN NOT NULL,
    diagnostic_category TEXT,
    resolution_status TEXT NOT NULL,
    resolution_note TEXT,
    calculation_trace TEXT NOT NULL,
    risk_dataset_id UUID,
    rule_version_id TEXT,
    calculation_id UUID,
    position_refs TEXT NOT NULL DEFAULT '',
    CONSTRAINT reconciliation_differences_currency_chk CHECK (currency = 'INR'),
    CONSTRAINT reconciliation_differences_resolution_chk CHECK (
        resolution_status IN ('unresolved', 'explained', 'accepted_exception')
    ),
    CONSTRAINT reconciliation_differences_material_chk CHECK (material = (absolute_difference <> 0)),
    CONSTRAINT reconciliation_differences_abs_chk CHECK (
        absolute_difference = abs(calculated_amount - expected_amount)
    ),
    CONSTRAINT reconciliation_differences_relative_chk CHECK (
        (expected_amount = 0 AND relative_difference IS NULL)
        OR (expected_amount <> 0 AND relative_difference IS NOT NULL)
    ),
    CONSTRAINT reconciliation_differences_category_chk CHECK (
        diagnostic_category IS NULL OR diagnostic_category IN (
            'wrong_contract_mapping',
            'missing_risk_array',
            'incorrect_quantity_normalization',
            'scenario_sign_or_order_mismatch',
            'incorrect_spread_eligibility',
            'incorrect_option_valuation',
            'elm_mismatch',
            'delivery_margin_mismatch',
            'incorrect_aggregation_scope',
            'incorrect_rounding',
            'outdated_or_wrong_snapshot',
            'unsupported_or_missing_exchange_rule'
        )
    )
);

CREATE INDEX reconciliation_differences_run_idx ON reconciliation_differences (run_id);

CREATE TRIGGER reconciliation_differences_protect
    BEFORE UPDATE ON reconciliation_differences
    FOR EACH ROW
    EXECUTE FUNCTION protect_reconciliation_differences();

CREATE TRIGGER reconciliation_differences_no_delete
    BEFORE DELETE ON reconciliation_differences
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TABLE audit_events (
    id UUID PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    detail JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX audit_events_entity_idx
    ON audit_events (entity_type, entity_id, occurred_at DESC);

CREATE TRIGGER audit_events_no_update
    BEFORE UPDATE ON audit_events
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();

CREATE TRIGGER audit_events_no_delete
    BEFORE DELETE ON audit_events
    FOR EACH ROW
    EXECUTE FUNCTION reject_row_mutation();
