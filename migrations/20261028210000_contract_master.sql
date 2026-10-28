-- NSE F&O contract master. The row stores the original gzip.
-- The quote reads MinLot and does not convert quantity.

ALTER TABLE official_auxiliary_files
    DROP CONSTRAINT official_auxiliary_files_kind_chk;

ALTER TABLE official_auxiliary_files
    ADD CONSTRAINT official_auxiliary_files_kind_chk CHECK (
        kind IN (
            'exposure_limit_csv',
            'contract_elm_gzip',
            'cash_var_dat',
            'contract_master_gzip'
        )
    );
