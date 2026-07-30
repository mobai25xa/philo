use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PerformanceBudgets {
    pub schema_version: u32,
    pub environment: EnvironmentBudget,
    pub microbenchmark: MicrobenchmarkBudget,
    pub benchmark_thresholds: BenchmarkThresholds,
    pub quick_soak: QuickSoakBudget,
    pub release_soak: ReleaseSoakBudget,
    pub connection_churn: ConnectionChurnBudget,
}

#[derive(Debug, Deserialize)]
pub struct EnvironmentBudget {
    pub required_repetitions: usize,
    pub baseline_max_age_days: u64,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
pub struct MicrobenchmarkBudget {
    pub smoke_cpu_iterations: usize,
    pub full_cpu_iterations: usize,
    pub smoke_async_iterations: usize,
    pub full_async_iterations: usize,
}

#[derive(Debug, Deserialize)]
pub struct QuickSoakBudget {
    pub default_iterations: usize,
    pub max_retained_captures: usize,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseSoakBudget {
    pub default_iterations: usize,
    pub resource_sample_interval: usize,
    pub linux_max_rss_growth_kib: i64,
    pub linux_max_thread_growth: i64,
    pub linux_max_file_descriptor_growth: i64,
    pub linux_max_socket_growth: i64,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionChurnBudget {
    pub default_iterations: usize,
}

#[derive(Debug, Deserialize)]
pub struct BenchmarkThresholds {
    pub status: String,
    pub warning_relative_regression_percent: u64,
    pub blocking_relative_regression_percent: u64,
}

pub struct PerformanceContext {
    schema_version: u32,
    suite: &'static str,
    candidate: String,
    rustc: String,
    required_repetitions: usize,
    baseline_max_age_seconds: u64,
    threshold_status: String,
    warning_percent: u64,
    blocking_percent: u64,
}

impl PerformanceContext {
    pub fn new(suite: &'static str, budgets: &PerformanceBudgets) -> Self {
        let candidate =
            std::env::var("PHILO_GATE_SUBJECT").unwrap_or_else(|_| "working-tree".to_owned());
        let rustc = Command::new("rustc")
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map_or_else(|| "unknown".to_owned(), |version| version.trim().to_owned());
        Self {
            schema_version: budgets.schema_version,
            suite,
            candidate,
            rustc,
            required_repetitions: budgets.environment.required_repetitions,
            baseline_max_age_seconds: budgets
                .environment
                .baseline_max_age_days
                .saturating_mul(24 * 60 * 60),
            threshold_status: budgets.benchmark_thresholds.status.clone(),
            warning_percent: budgets
                .benchmark_thresholds
                .warning_relative_regression_percent,
            blocking_percent: budgets
                .benchmark_thresholds
                .blocking_relative_regression_percent,
        }
    }

    pub fn json_fields(&self) -> String {
        format!(
            "\"schema\":\"philo/performance-report\",\"schema_version\":{},\"suite\":\"{}\",\"candidate\":{},\"generated_unix_seconds\":{},\"os\":\"{}\",\"arch\":\"{}\",\"rustc\":{},\"profile\":\"release\",\"features\":\"{}\"",
            self.schema_version,
            self.suite,
            json_string(&self.candidate),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_secs(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            json_string(&self.rustc),
            feature_set(),
        )
    }

    pub fn print_metric(&self, metric: &str, unit: &str, value: u128, iterations: usize) {
        self.check_baseline(metric, unit, value);
        println!(
            "{{{},\"metric\":{},\"unit\":{},\"value\":{value},\"iterations\":{iterations}}}",
            self.json_fields(),
            json_string(metric),
            json_string(unit),
        );
    }

    fn check_baseline(&self, metric: &str, unit: &str, value: u128) {
        let Some(path) = std::env::var_os("PHILO_PERFORMANCE_BASELINE") else {
            return;
        };
        let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "failed to read performance baseline {}: {error}",
                PathBuf::from(path).display()
            )
        });
        assert_eq!(
            self.threshold_status, "approved",
            "performance thresholds must be approved before baseline comparison"
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before Unix epoch")
            .as_secs();
        let mut baseline_candidate = None::<String>;
        let mut values = source
            .lines()
            .filter_map(|line| serde_json::from_str::<BaselineMetric>(line).ok())
            .filter(|baseline| baseline.metric == metric && baseline.unit == unit)
            .map(|baseline| {
                assert_eq!(baseline.schema, "philo/performance-report");
                assert_eq!(baseline.schema_version, self.schema_version);
                assert_eq!(baseline.suite, self.suite);
                assert_eq!(baseline.os, std::env::consts::OS);
                assert_eq!(baseline.arch, std::env::consts::ARCH);
                assert_eq!(baseline.rustc, self.rustc);
                assert_eq!(baseline.profile, "release");
                assert_eq!(baseline.features, feature_set());
                assert!(
                    baseline.candidate.len() == 40
                        && baseline
                            .candidate
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit()),
                    "baseline candidate must be a full commit SHA"
                );
                if let Some(candidate) = &baseline_candidate {
                    assert_eq!(candidate, &baseline.candidate, "baseline mixes candidates");
                } else {
                    baseline_candidate = Some(baseline.candidate.clone());
                }
                assert!(
                    baseline.generated_unix_seconds <= now.saturating_add(300),
                    "baseline timestamp is unreasonably far in the future"
                );
                assert!(
                    now.saturating_sub(baseline.generated_unix_seconds)
                        <= self.baseline_max_age_seconds,
                    "baseline is older than the configured maximum age"
                );
                baseline.value
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values.len(),
            self.required_repetitions,
            "baseline metric {metric} must have exactly {} comparable repetitions",
            self.required_repetitions
        );
        values.sort_unstable();
        let baseline = values[values.len() / 2];
        assert_ne!(baseline, 0, "baseline metric {metric} must be positive");
        let regression_basis_points =
            value.saturating_sub(baseline).saturating_mul(10_000) / baseline;
        if regression_basis_points >= u128::from(self.warning_percent) * 100 {
            eprintln!(
                "performance warning: {metric} regressed {}.{:02}% (warning {}%, blocking {}%)",
                regression_basis_points / 100,
                regression_basis_points % 100,
                self.warning_percent,
                self.blocking_percent
            );
        }
        assert!(
            regression_basis_points < u128::from(self.blocking_percent) * 100,
            "performance regression: {metric} regressed {}.{:02}% (blocking {}%)",
            regression_basis_points / 100,
            regression_basis_points % 100,
            self.blocking_percent,
        );
    }
}

#[derive(Deserialize)]
struct BaselineMetric {
    schema: String,
    schema_version: u32,
    suite: String,
    os: String,
    arch: String,
    rustc: String,
    profile: String,
    features: String,
    candidate: String,
    generated_unix_seconds: u64,
    metric: String,
    unit: String,
    value: u128,
}

pub fn load_budgets() -> PerformanceBudgets {
    let path = std::env::var_os("PHILO_PERFORMANCE_BUDGETS").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("support")
                .join("performance-budgets.toml")
        },
        PathBuf::from,
    );
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    toml::from_str(&source)
        .unwrap_or_else(|error| panic!("invalid performance budget {}: {error}", path.display()))
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization is infallible")
}

fn feature_set() -> &'static str {
    match (cfg!(feature = "rustls-tls"), cfg!(feature = "tracing")) {
        (true, true) => "rustls-tls,tracing",
        (true, false) => "rustls-tls",
        (false, true) => "tracing",
        (false, false) => "none",
    }
}
