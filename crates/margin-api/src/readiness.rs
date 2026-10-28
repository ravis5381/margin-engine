use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReadinessCheck {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadinessReport {
    pub ready: bool,
    pub checks: Vec<ReadinessCheck>,
}

pub struct ReadinessProbe {
    pub database_ok: bool,
    pub database_detail: String,
    pub official_validated_datasets: Option<u64>,
    pub official_calculation_implemented: bool,
}

pub fn evaluate_readiness(probe: ReadinessProbe) -> ReadinessReport {
    let dataset_passed = probe
        .official_validated_datasets
        .is_some_and(|count| count > 0);
    let dataset_detail = match probe.official_validated_datasets {
        Some(0) => "no published official dataset is available".to_owned(),
        Some(count) => format!("{count} published official dataset(s) are available"),
        None if probe.database_ok => "validated dataset count could not be read".to_owned(),
        None => "not checked because the database is unavailable".to_owned(),
    };
    let calculation_detail = if probe.official_calculation_implemented {
        "an official margin calculation is implemented".to_owned()
    } else {
        "no official margin calculation is implemented; diagnostic scenario aggregation is not a production calculation".to_owned()
    };
    let checks = vec![
        ReadinessCheck {
            name: "database",
            passed: probe.database_ok,
            detail: probe.database_detail,
        },
        ReadinessCheck {
            name: "official_validated_dataset",
            passed: dataset_passed,
            detail: dataset_detail,
        },
        ReadinessCheck {
            name: "official_calculation",
            passed: probe.official_calculation_implemented,
            detail: calculation_detail,
        },
    ];
    let ready = checks.iter().all(|check| check.passed);
    ReadinessReport { ready, checks }
}
