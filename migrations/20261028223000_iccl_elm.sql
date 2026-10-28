-- Daily ICCL extreme-loss zip from www.bseindia.com.

ALTER TABLE official_auxiliary_files
    DROP CONSTRAINT official_auxiliary_files_kind_chk;

ALTER TABLE official_auxiliary_files
    ADD CONSTRAINT official_auxiliary_files_kind_chk CHECK (
        kind IN (
            'exposure_limit_csv',
            'contract_elm_gzip',
            'cash_var_dat',
            'contract_master_gzip',
            'iccl_elm_zip'
        )
    );
