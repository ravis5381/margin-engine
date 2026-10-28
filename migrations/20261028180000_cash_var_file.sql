-- Capital-market VaR file used for equity-derivatives delivery margin.
-- The row stores the original bytes. The quote reads the EQ daily margin rate.

ALTER TABLE official_auxiliary_files
    DROP CONSTRAINT official_auxiliary_files_kind_chk;

ALTER TABLE official_auxiliary_files
    ADD CONSTRAINT official_auxiliary_files_kind_chk CHECK (
        kind IN ('exposure_limit_csv', 'contract_elm_gzip', 'cash_var_dat')
    );
