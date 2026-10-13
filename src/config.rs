use std::collections::BTreeMap;
use std::fmt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::guard::GuardConfig;
use crate::types::{Action, Policy};

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Parse(String),
    Validation { key: String, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Parse(err) => write!(f, "{err}"),
            ConfigError::Validation { key, message } => write!(f, "{key}: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DetectorPolicyOverride {
    pub interval: Option<Duration>,
    pub timeout: Option<Duration>,
    pub threshold: Option<u32>,
    pub actions: Option<Vec<Action>>,
}

impl DetectorPolicyOverride {
    pub fn is_empty(&self) -> bool {
        self.interval.is_none()
            && self.timeout.is_none()
            && self.threshold.is_none()
            && self.actions.is_none()
    }

    pub fn apply_to(&self, base: &Policy) -> Policy {
        Policy {
            interval: self.interval.unwrap_or(base.interval),
            timeout: self.timeout.unwrap_or(base.timeout),
            threshold: self.threshold.unwrap_or(base.threshold),
            actions: self.actions.clone().unwrap_or_else(|| base.actions.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub dry_run: bool,
    pub once: bool,
    pub detectors_dir: PathBuf,
    pub state_dir: PathBuf,
    pub reboot: GuardConfig,
    pub defaults: Policy,
    pub detectors: BTreeMap<String, DetectorPolicyOverride>,
    pub loaded_from: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dry_run: false,
            once: false,
            detectors_dir: PathBuf::from("/etc/failsafe/detectors.d"),
            state_dir: PathBuf::from("/var/lib/failsafe"),
            reboot: GuardConfig::default(),
            defaults: Policy::default(),
            detectors: BTreeMap::new(),
            loaded_from: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    dry_run: Option<bool>,
    #[serde(default)]
    detectors_dir: Option<PathBuf>,
    #[serde(default)]
    state_dir: Option<PathBuf>,
    #[serde(default)]
    reboot: Option<RawReboot>,
    #[serde(default)]
    defaults: Option<RawPolicy>,
    #[serde(default)]
    detector: Option<BTreeMap<String, RawPolicy>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReboot {
    #[serde(default)]
    boot_grace_secs: Option<u64>,
    #[serde(default)]
    backoff_base_secs: Option<u64>,
    #[serde(default)]
    backoff_max_secs: Option<u64>,
    #[serde(default)]
    stable_after_secs: Option<u64>,
    #[serde(default)]
    reboot_deadline_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    #[serde(default)]
    interval_secs: Option<u64>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    threshold: Option<u32>,
    #[serde(default)]
    actions: Option<Vec<String>>,
}

pub const MAX_DURATION_SECS: u64 = 31_536_000;

fn parse_duration(secs: u64, key: &str, allow_zero: bool) -> Result<Duration, ConfigError> {
    if !allow_zero && secs == 0 {
        return Err(ConfigError::Validation {
            key: key.to_string(),
            message: "duration must be greater than 0".to_string(),
        });
    }
    if secs > MAX_DURATION_SECS {
        return Err(ConfigError::Validation {
            key: key.to_string(),
            message: format!("duration must not exceed {MAX_DURATION_SECS} seconds (365 days)"),
        });
    }
    Ok(Duration::from_secs(secs))
}

fn parse_actions(actions: Vec<String>, key: &str) -> Result<Vec<Action>, ConfigError> {
    if actions.is_empty() {
        return Err(ConfigError::Validation {
            key: key.to_string(),
            message: "actions must not be empty".to_string(),
        });
    }
    let mut parsed = Vec::with_capacity(actions.len());
    for act in actions {
        match act.as_str() {
            "reboot" => parsed.push(Action::Reboot),
            other => {
                return Err(ConfigError::Validation {
                    key: key.to_string(),
                    message: format!("unknown action '{other}'; only 'reboot' is supported"),
                });
            }
        }
    }
    Ok(parsed)
}

fn parse_threshold(threshold: u32, key: &str) -> Result<u32, ConfigError> {
    if threshold == 0 {
        return Err(ConfigError::Validation {
            key: key.to_string(),
            message: "threshold must be at least 1".to_string(),
        });
    }
    Ok(threshold)
}

impl Config {
    pub fn parse(toml_str: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig =
            toml::from_str(toml_str).map_err(|e| ConfigError::Parse(e.to_string()))?;

        let mut config = Config::default();

        if let Some(dry_run) = raw.dry_run {
            config.dry_run = dry_run;
        }

        if let Some(detectors_dir) = raw.detectors_dir {
            if !detectors_dir.is_absolute() {
                return Err(ConfigError::Validation {
                    key: "detectors_dir".to_string(),
                    message: "must be an absolute path".to_string(),
                });
            }
            config.detectors_dir = detectors_dir;
        }

        if let Some(state_dir) = raw.state_dir {
            if !state_dir.is_absolute() {
                return Err(ConfigError::Validation {
                    key: "state_dir".to_string(),
                    message: "must be an absolute path".to_string(),
                });
            }
            config.state_dir = state_dir;
        }

        if let Some(reboot) = raw.reboot {
            if let Some(val) = reboot.boot_grace_secs {
                config.reboot.boot_grace = parse_duration(val, "reboot.boot_grace_secs", true)?;
            }
            if let Some(val) = reboot.backoff_base_secs {
                config.reboot.backoff_base = parse_duration(val, "reboot.backoff_base_secs", false)?;
            }
            if let Some(val) = reboot.backoff_max_secs {
                config.reboot.backoff_max = parse_duration(val, "reboot.backoff_max_secs", false)?;
            }
            if let Some(val) = reboot.stable_after_secs {
                config.reboot.stable_after = parse_duration(val, "reboot.stable_after_secs", false)?;
            }
            if let Some(val) = reboot.reboot_deadline_secs {
                config.reboot.reboot_deadline =
                    parse_duration(val, "reboot.reboot_deadline_secs", false)?;
            }
        }

        if config.reboot.backoff_max < config.reboot.backoff_base {
            return Err(ConfigError::Validation {
                key: "reboot.backoff_max_secs".to_string(),
                message: "must not be less than backoff_base_secs".to_string(),
            });
        }

        if config.reboot.stable_after < config.reboot.boot_grace + config.reboot.backoff_base {
            return Err(ConfigError::Validation {
                key: "reboot.stable_after_secs".to_string(),
                message: "must not be less than boot_grace_secs + backoff_base_secs".to_string(),
            });
        }

        if let Some(defaults) = raw.defaults {
            if let Some(val) = defaults.interval_secs {
                config.defaults.interval = parse_duration(val, "defaults.interval_secs", false)?;
            }
            if let Some(val) = defaults.timeout_secs {
                config.defaults.timeout = parse_duration(val, "defaults.timeout_secs", false)?;
            }
            if let Some(val) = defaults.threshold {
                config.defaults.threshold = parse_threshold(val, "defaults.threshold")?;
            }
            if let Some(actions) = defaults.actions {
                config.defaults.actions = parse_actions(actions, "defaults.actions")?;
            }
        }

        if let Some(detectors) = raw.detector {
            for (name, raw_override) in detectors {
                if name.is_empty() {
                    return Err(ConfigError::Validation {
                        key: "detector".to_string(),
                        message: "detector name must not be empty".to_string(),
                    });
                }
                let mut ovr = DetectorPolicyOverride::default();
                if let Some(val) = raw_override.interval_secs {
                    ovr.interval = Some(parse_duration(
                        val,
                        &format!("detector.{name}.interval_secs"),
                        false,
                    )?);
                }
                if let Some(val) = raw_override.timeout_secs {
                    ovr.timeout = Some(parse_duration(
                        val,
                        &format!("detector.{name}.timeout_secs"),
                        false,
                    )?);
                }
                if let Some(val) = raw_override.threshold {
                    ovr.threshold =
                        Some(parse_threshold(val, &format!("detector.{name}.threshold"))?);
                }
                if let Some(actions) = raw_override.actions {
                    ovr.actions = Some(parse_actions(
                        actions,
                        &format!("detector.{name}.actions"),
                    )?);
                }
                config.detectors.insert(name, ovr);
            }
        }

        Ok(config)
    }

    pub fn policy_for(&self, name: &str) -> Policy {
        if let Some(ovr) = self.detectors.get(name) {
            ovr.apply_to(&self.defaults)
        } else {
            self.defaults.clone()
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoadError {
    Help,
    Exit(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Help => write!(f, "help requested"),
            LoadError::Exit(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for LoadError {}

pub fn print_usage() {
    eprintln!(
        "Usage: failsafe [--config PATH] [--dry-run] [--once]\n\n\
         Options:\n  \
           --config PATH   Path to configuration file (default: /etc/failsafe/failsafe.toml)\n  \
           --dry-run       Do not reboot; log what would have been done\n  \
           --once          Run each detector once, report outcomes, and exit\n  \
           -h, --help      Show this help message"
    );
}

fn load_file(path: &Path, euid: u32, optional: bool) -> Result<Option<Config>, LoadError> {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LoadError::Exit(format!(
                "cannot read configuration file {}: {e}",
                path.display()
            )));
        }
    };
    let meta = file.metadata().map_err(|e| {
        LoadError::Exit(format!(
            "cannot read configuration file {}: {e}",
            path.display()
        ))
    })?;
    crate::detector::check_permissions(euid, meta.uid(), meta.mode())
        .map_err(|e| LoadError::Exit(format!("config file {}: {e}", path.display())))?;
    let mut content = String::new();
    file.read_to_string(&mut content).map_err(|e| {
        LoadError::Exit(format!(
            "cannot read configuration file {}: {e}",
            path.display()
        ))
    })?;
    let cfg = Config::parse(&content)
        .map_err(|e| LoadError::Exit(format!("config file {}: {e}", path.display())))?;
    Ok(Some(cfg))
}

pub fn load(args: impl IntoIterator<Item = String>) -> Result<Config, LoadError> {
    let euid = unsafe { libc::geteuid() };
    load_with_euid(args, euid)
}

pub fn load_with_euid(
    args: impl IntoIterator<Item = String>,
    euid: u32,
) -> Result<Config, LoadError> {
    let mut config_path: Option<PathBuf> = None;
    let mut cli_dry_run = false;
    let mut cli_once = false;

    let args: Vec<String> = args.into_iter().collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            return Err(LoadError::Help);
        } else if arg == "--dry-run" {
            cli_dry_run = true;
        } else if arg == "--once" {
            cli_once = true;
        } else if arg == "--config" {
            i += 1;
            if i >= args.len() {
                return Err(LoadError::Exit("missing argument for --config".to_string()));
            }
            config_path = Some(PathBuf::from(&args[i]));
        } else if let Some(val) = arg.strip_prefix("--config=") {
            config_path = Some(PathBuf::from(val));
        } else {
            return Err(LoadError::Exit(format!("unknown argument: {arg}")));
        }
        i += 1;
    }

    let default_path = PathBuf::from("/etc/failsafe/failsafe.toml");
    let (mut config, loaded_path) = match config_path {
        Some(path) => {
            let parsed = load_file(&path, euid, false)?.unwrap();
            (parsed, Some(path))
        }
        None => match load_file(&default_path, euid, true)? {
            Some(parsed) => (parsed, Some(default_path)),
            None => (Config::default(), None),
        },
    };

    config.loaded_from = loaded_path;
    if cli_dry_run {
        config.dry_run = true;
    }
    if cli_once {
        config.once = true;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_text_equals_all_defaults() {
        let empty = Config::parse("").unwrap();
        assert_eq!(empty, Config::default());

        let comments_only = Config::parse(
            r#"
            # Just comments and whitespace
            # nothing configured
            "#,
        )
        .unwrap();
        assert_eq!(comments_only, Config::default());
    }

    #[test]
    fn test_architecture_document_example() {
        let toml_str = r#"
        dry_run        = false
        detectors_dir  = "/etc/failsafe/detectors.d"
        state_dir      = "/var/lib/failsafe"

        [reboot]
        boot_grace_secs      = 300
        backoff_base_secs    = 600
        backoff_max_secs     = 86400
        stable_after_secs    = 1800
        reboot_deadline_secs = 120

        [defaults]                 # policy for any detector without its own table
        interval_secs = 60
        timeout_secs  = 30
        threshold     = 3
        actions       = ["reboot"]

        [detector."lte-link"]      # key is the detector's file name; overrides defaults per field
        threshold = 5
        "#;

        let cfg = Config::parse(toml_str).unwrap();
        assert!(!cfg.dry_run);
        assert_eq!(
            cfg.detectors_dir,
            PathBuf::from("/etc/failsafe/detectors.d")
        );
        assert_eq!(cfg.state_dir, PathBuf::from("/var/lib/failsafe"));
        assert_eq!(cfg.reboot, GuardConfig::default());
        assert_eq!(cfg.defaults, Policy::default());

        let lte_policy = cfg.policy_for("lte-link");
        assert_eq!(lte_policy.threshold, 5);
        assert_eq!(lte_policy.interval, Duration::from_secs(60));
        assert_eq!(lte_policy.timeout, Duration::from_secs(30));
        assert_eq!(lte_policy.actions, vec![Action::Reboot]);

        let default_det_policy = cfg.policy_for("other-detector");
        assert_eq!(default_det_policy, Policy::default());
    }

    #[test]
    fn test_per_detector_table_overrides_only_named_fields() {
        let toml_str = r#"
        [defaults]
        interval_secs = 10
        timeout_secs  = 5
        threshold     = 2
        actions       = ["reboot"]

        [detector.alpha]
        interval_secs = 20

        [detector.beta]
        threshold = 9
        "#;

        let cfg = Config::parse(toml_str).unwrap();

        let alpha = cfg.policy_for("alpha");
        assert_eq!(alpha.interval, Duration::from_secs(20));
        assert_eq!(alpha.timeout, Duration::from_secs(5));
        assert_eq!(alpha.threshold, 2);
        assert_eq!(alpha.actions, vec![Action::Reboot]);

        let beta = cfg.policy_for("beta");
        assert_eq!(beta.interval, Duration::from_secs(10));
        assert_eq!(beta.timeout, Duration::from_secs(5));
        assert_eq!(beta.threshold, 9);
        assert_eq!(beta.actions, vec![Action::Reboot]);

        let gamma = cfg.policy_for("gamma");
        assert_eq!(gamma.interval, Duration::from_secs(10));
        assert_eq!(gamma.timeout, Duration::from_secs(5));
        assert_eq!(gamma.threshold, 2);
        assert_eq!(gamma.actions, vec![Action::Reboot]);
    }

    #[test]
    fn test_zero_boot_grace_allowed() {
        let toml_str = r#"
        [reboot]
        boot_grace_secs = 0
        "#;

        let cfg = Config::parse(toml_str).unwrap();
        assert_eq!(cfg.reboot.boot_grace, Duration::ZERO);
    }

    #[test]
    fn test_rejected_decimal_duration() {
        let err = Config::parse("[defaults]\ninterval_secs = 0.5").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));

        let err2 = Config::parse("[reboot]\nboot_grace_secs = 1e-12").unwrap_err();
        assert!(matches!(err2, ConfigError::Parse(_)));
    }

    #[test]
    fn test_duration_at_max_limit_allowed() {
        let toml_str = format!(
            r#"
            [reboot]
            boot_grace_secs = 0
            backoff_base_secs = 1000
            stable_after_secs = {MAX_DURATION_SECS}
            "#
        );
        let cfg = Config::parse(&toml_str).unwrap();
        assert_eq!(
            cfg.reboot.stable_after,
            Duration::from_secs(MAX_DURATION_SECS)
        );
    }

    #[test]
    fn test_rejected_duration_exceeding_max_limit() {
        // Just over the limit: 31_536_001
        let err = Config::parse(&format!("[defaults]\ninterval_secs = {}", MAX_DURATION_SECS + 1))
            .unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "defaults.interval_secs");
                assert!(message.contains("must not exceed 31536000"));
            }
            _ => panic!("unexpected error type: {err}"),
        }

        // Huge value from C5
        let err_huge = Config::parse("[detector.det]\ntimeout_secs = 9223372036854775807")
            .unwrap_err();
        match err_huge {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "detector.det.timeout_secs");
                assert!(message.contains("must not exceed 31536000"));
            }
            _ => panic!("unexpected error type: {err_huge}"),
        }
    }

    // --- Tests for rejected inputs ---

    #[test]
    fn test_rejected_unknown_key_top_level() {
        let err = Config::parse("unknown_key = 123").unwrap_err();
        assert!(err.to_string().contains("unknown_key"));
    }

    #[test]
    fn test_rejected_unknown_table_top_level() {
        let err = Config::parse("[foo]\nbar = 1").unwrap_err();
        assert!(err.to_string().contains("foo"));
    }

    #[test]
    fn test_rejected_unknown_key_in_reboot() {
        let err = Config::parse("[reboot]\nunknown_key = 123").unwrap_err();
        assert!(err.to_string().contains("unknown_key"));
    }

    #[test]
    fn test_rejected_unknown_key_in_defaults() {
        let err = Config::parse("[defaults]\nunknown_key = 123").unwrap_err();
        assert!(err.to_string().contains("unknown_key"));
    }

    #[test]
    fn test_rejected_unknown_key_in_detector() {
        let err = Config::parse("[detector.my_det]\nunknown_key = 123").unwrap_err();
        assert!(err.to_string().contains("unknown_key"));
    }

    #[test]
    fn test_rejected_wrong_type_top_level() {
        let err = Config::parse("dry_run = \"true\"").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));

        let err2 = Config::parse("detectors_dir = 123").unwrap_err();
        assert!(matches!(err2, ConfigError::Parse(_)));
    }

    #[test]
    fn test_rejected_wrong_type_reboot() {
        let err = Config::parse("[reboot]\nboot_grace_secs = \"instant\"").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn test_rejected_wrong_type_defaults() {
        let err = Config::parse("[defaults]\nthreshold = \"three\"").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));

        let err2 = Config::parse("[defaults]\nactions = \"reboot\"").unwrap_err();
        assert!(matches!(err2, ConfigError::Parse(_)));
    }

    #[test]
    fn test_rejected_zero_duration_defaults_interval() {
        let err = Config::parse("[defaults]\ninterval_secs = 0").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "defaults.interval_secs");
                assert!(message.contains("greater than 0"));
            }
            _ => panic!("unexpected error type: {err}"),
        }
    }

    #[test]
    fn test_rejected_zero_duration_defaults_timeout() {
        let err = Config::parse("[defaults]\ntimeout_secs = 0").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "defaults.timeout_secs");
                assert!(message.contains("greater than 0"));
            }
            _ => panic!("unexpected error type: {err}"),
        }
    }

    #[test]
    fn test_rejected_zero_duration_reboot_backoff_base() {
        let err = Config::parse("[reboot]\nbackoff_base_secs = 0").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.backoff_base_secs");
                assert!(message.contains("greater than 0"));
            }
            _ => panic!("unexpected error type: {err}"),
        }
    }

    #[test]
    fn test_rejected_zero_duration_detector_interval() {
        let err = Config::parse("[detector.my_det]\ninterval_secs = 0").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "detector.my_det.interval_secs");
                assert!(message.contains("greater than 0"));
            }
            _ => panic!("unexpected error type: {err}"),
        }
    }

    #[test]
    fn test_rejected_negative_duration() {
        let err = Config::parse("[defaults]\ninterval_secs = -1").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));

        let err2 = Config::parse("[reboot]\nboot_grace_secs = -5").unwrap_err();
        assert!(matches!(err2, ConfigError::Parse(_)));
    }

    #[test]
    fn test_rejected_non_finite_duration() {
        let err = Config::parse("[defaults]\ninterval_secs = nan").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));

        let err2 = Config::parse("[reboot]\nbackoff_max_secs = inf").unwrap_err();
        assert!(matches!(err2, ConfigError::Parse(_)));
    }

    #[test]
    fn test_rejected_stable_after_below_boot_grace_plus_backoff_base() {
        // Explicit values: 300 + 600 = 900 > 800
        let err = Config::parse(
            r#"
            [reboot]
            boot_grace_secs = 300
            backoff_base_secs = 600
            stable_after_secs = 800
            "#,
        )
        .unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.stable_after_secs");
                assert!(message.contains("must not be less than boot_grace_secs + backoff_base_secs"));
            }
            _ => panic!("unexpected error type: {err}"),
        }

        // Only stable_after_secs explicit (800 < default 300 + 600 = 900)
        let err_only_stable = Config::parse(
            r#"
            [reboot]
            stable_after_secs = 800
            "#,
        )
        .unwrap_err();
        match err_only_stable {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.stable_after_secs");
                assert!(message.contains("must not be less than boot_grace_secs + backoff_base_secs"));
            }
            _ => panic!("unexpected error type: {err_only_stable}"),
        }

        // Only boot_grace_secs explicit (1500 + default 600 = 2100 > default 1800)
        let err_only_grace = Config::parse(
            r#"
            [reboot]
            boot_grace_secs = 1500
            "#,
        )
        .unwrap_err();
        match err_only_grace {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.stable_after_secs");
                assert!(message.contains("must not be less than boot_grace_secs + backoff_base_secs"));
            }
            _ => panic!("unexpected error type: {err_only_grace}"),
        }

        // Only backoff_base_secs explicit (default 300 + 1600 = 1900 > default 1800)
        let err_only_base = Config::parse(
            r#"
            [reboot]
            backoff_base_secs = 1600
            "#,
        )
        .unwrap_err();
        match err_only_base {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.stable_after_secs");
                assert!(message.contains("must not be less than boot_grace_secs + backoff_base_secs"));
            }
            _ => panic!("unexpected error type: {err_only_base}"),
        }

        // Equal is allowed (300 + 600 = 900 == 900)
        let ok_equal = Config::parse(
            r#"
            [reboot]
            boot_grace_secs = 300
            backoff_base_secs = 600
            stable_after_secs = 900
            "#,
        );
        assert!(ok_equal.is_ok());
    }

    #[test]
    fn test_rejected_zero_threshold() {
        let err = Config::parse("[defaults]\nthreshold = 0").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "defaults.threshold");
                assert!(message.contains("at least 1"));
            }
            _ => panic!("unexpected error type: {err}"),
        }

        let err2 = Config::parse("[detector.det]\nthreshold = 0").unwrap_err();
        match err2 {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "detector.det.threshold");
                assert!(message.contains("at least 1"));
            }
            _ => panic!("unexpected error type: {err2}"),
        }
    }

    #[test]
    fn test_rejected_empty_action() {
        let err = Config::parse("[defaults]\nactions = []").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "defaults.actions");
                assert!(message.contains("must not be empty"));
            }
            _ => panic!("unexpected error type: {err}"),
        }

        let err2 = Config::parse("[detector.det]\nactions = []").unwrap_err();
        match err2 {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "detector.det.actions");
                assert!(message.contains("must not be empty"));
            }
            _ => panic!("unexpected error type: {err2}"),
        }
    }

    #[test]
    fn test_rejected_unknown_action() {
        let err = Config::parse("[defaults]\nactions = [\"poweroff\"]").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "defaults.actions");
                assert!(message.contains("unknown action 'poweroff'"));
            }
            _ => panic!("unexpected error type: {err}"),
        }

        let err2 = Config::parse("[detector.det]\nactions = [\"reboot\", \"custom\"]").unwrap_err();
        match err2 {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "detector.det.actions");
                assert!(message.contains("unknown action 'custom'"));
            }
            _ => panic!("unexpected error type: {err2}"),
        }
    }

    #[test]
    fn test_rejected_backoff_max_below_backoff_base() {
        // Both explicit
        let err = Config::parse(
            r#"
            [reboot]
            backoff_base_secs = 600
            backoff_max_secs = 300
            "#,
        )
        .unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.backoff_max_secs");
                assert!(message.contains("not be less than backoff_base_secs"));
            }
            _ => panic!("unexpected error type: {err}"),
        }

        // Only max explicit, below default base (600)
        let err2 = Config::parse(
            r#"
            [reboot]
            backoff_max_secs = 100
            "#,
        )
        .unwrap_err();
        match err2 {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.backoff_max_secs");
                assert!(message.contains("not be less than backoff_base_secs"));
            }
            _ => panic!("unexpected error type: {err2}"),
        }

        // Only base explicit, above default max (86400)
        let err3 = Config::parse(
            r#"
            [reboot]
            backoff_base_secs = 100000
            "#,
        )
        .unwrap_err();
        match err3 {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "reboot.backoff_max_secs");
                assert!(message.contains("not be less than backoff_base_secs"));
            }
            _ => panic!("unexpected error type: {err3}"),
        }
    }

    #[test]
    fn test_rejected_relative_paths() {
        let err = Config::parse("detectors_dir = \"relative/path\"").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "detectors_dir");
                assert!(message.contains("absolute path"));
            }
            _ => panic!("unexpected error type: {err}"),
        }

        let err2 = Config::parse("state_dir = \"relative/path\"").unwrap_err();
        match err2 {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "state_dir");
                assert!(message.contains("absolute path"));
            }
            _ => panic!("unexpected error type: {err2}"),
        }
    }

    #[test]
    fn test_rejected_empty_detector_name() {
        let err = Config::parse("[detector.\"\"]\nthreshold = 5").unwrap_err();
        match err {
            ConfigError::Validation { key, message } => {
                assert_eq!(key, "detector");
                assert!(message.contains("empty"));
            }
            _ => panic!("unexpected error type: {err}"),
        }
    }

    #[test]
    fn test_load_help_flags() {
        assert_eq!(load_with_euid(vec!["-h".to_string()], 1000), Err(LoadError::Help));
        assert_eq!(
            load_with_euid(vec!["--help".to_string()], 1000),
            Err(LoadError::Help)
        );
    }

    #[test]
    fn test_load_unknown_argument() {
        let err = load_with_euid(vec!["--bad-flag".to_string()], 1000).unwrap_err();
        assert_eq!(err, LoadError::Exit("unknown argument: --bad-flag".to_string()));
    }

    #[test]
    fn test_load_missing_config_argument() {
        let err = load_with_euid(vec!["--config".to_string()], 1000).unwrap_err();
        assert_eq!(
            err,
            LoadError::Exit("missing argument for --config".to_string())
        );
    }

    #[test]
    fn test_load_explicit_missing_file_errors() {
        let err = load_with_euid(
            vec!["--config".to_string(), "/nonexistent/path/cfg.toml".to_string()],
            1000,
        )
        .unwrap_err();
        match err {
            LoadError::Exit(msg) => {
                assert!(msg.contains("cannot read configuration file"));
                assert!(msg.contains("/nonexistent/path/cfg.toml"));
            }
            _ => panic!("unexpected error type: {err:?}"),
        }
    }

    #[test]
    fn test_load_explicit_file_and_cli_flags() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!("failsafe_test_load_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let cfg_path = tmp.join("test.toml");

        fs::write(
            &cfg_path,
            r#"
            dry_run = false
            [defaults]
            interval_secs = 15
            "#,
        )
        .unwrap();
        fs::set_permissions(&cfg_path, fs::Permissions::from_mode(0o644)).unwrap();

        // 1. Load without CLI flags: dry_run matches file (false), once is false
        let cfg1 = load_with_euid(
            vec![format!("--config={}", cfg_path.display())],
            1000,
        )
        .unwrap();
        assert!(!cfg1.dry_run);
        assert!(!cfg1.once);
        assert_eq!(cfg1.defaults.interval, Duration::from_secs(15));
        assert_eq!(cfg1.loaded_from, Some(cfg_path.clone()));

        // 2. Load with --dry-run and --once: both enabled
        let cfg2 = load_with_euid(
            vec![
                "--config".to_string(),
                cfg_path.to_string_lossy().into_owned(),
                "--dry-run".to_string(),
                "--once".to_string(),
            ],
            1000,
        )
        .unwrap();
        assert!(cfg2.dry_run);
        assert!(cfg2.once);

        // 3. File with dry_run = true cannot be turned off by CLI
        fs::write(
            &cfg_path,
            r#"
            dry_run = true
            "#,
        )
        .unwrap();
        let cfg3 = load_with_euid(
            vec![format!("--config={}", cfg_path.display())],
            1000,
        )
        .unwrap();
        assert!(cfg3.dry_run);

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn test_load_root_ownership_enforcement() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!("failsafe_test_owner_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let cfg_path = tmp.join("root_check.toml");

        fs::write(&cfg_path, "dry_run = true\n").unwrap();

        // When euid != 0 (non-root developer run): permissions check skipped
        let res_non_root = load_with_euid(
            vec![format!("--config={}", cfg_path.display())],
            1000,
        );
        assert!(res_non_root.is_ok());

        // When euid == 0:
        // Current user in unprivileged test is not root (uid != 0), so check fails
        let res_root = load_with_euid(
            vec![format!("--config={}", cfg_path.display())],
            0,
        );
        assert!(res_root.is_err());
        match res_root.unwrap_err() {
            LoadError::Exit(msg) => {
                assert!(msg.contains("must be owned by root"));
            }
            _ => panic!("expected LoadError::Exit"),
        }

        // Test group-writable mode failure when euid == 0
        fs::set_permissions(&cfg_path, fs::Permissions::from_mode(0o664)).unwrap();
        let res_root_perm = load_with_euid(
            vec![format!("--config={}", cfg_path.display())],
            0,
        );
        assert!(res_root_perm.is_err());

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn test_shipped_config_file_equals_defaults() {
        let shipped = include_str!("../failsafe.toml");
        let parsed = Config::parse(shipped).expect("shipped failsafe.toml must parse cleanly");
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn test_shipped_config_file_uncommented_equals_defaults() {
        let shipped = include_str!("../failsafe.toml");
        let mut uncommented = String::new();
        let mut in_detector_example = false;

        for line in shipped.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("# [detector.") {
                in_detector_example = true;
            }
            if !in_detector_example
                && let Some(rest) = trimmed.strip_prefix("# ")
                && (rest.starts_with('[') || rest.contains(" = "))
            {
                uncommented.push_str(rest);
                uncommented.push('\n');
            }
        }

        let parsed = Config::parse(&uncommented)
            .expect("uncommented failsafe.toml defaults must parse cleanly");
        assert_eq!(parsed, Config::default());

        // Verify that the commented detector example uncomments and parses validly
        let mut with_example = uncommented.clone();
        for line in shipped.lines() {
            let trimmed = line.trim();
            if (trimmed.starts_with("# [detector.")
                || (trimmed.starts_with("# threshold = 5") && trimmed.contains('=')))
                && let Some(rest) = trimmed.strip_prefix("# ")
            {
                with_example.push_str(rest);
                with_example.push('\n');
            }
        }
        let parsed_example = Config::parse(&with_example)
            .expect("uncommented example detector must parse cleanly");
        assert_eq!(parsed_example.policy_for("example-detector").threshold, 5);

        // Verify that every key from the architecture design is explicitly present
        assert!(shipped.contains("dry_run"));
        assert!(shipped.contains("detectors_dir"));
        assert!(shipped.contains("state_dir"));
        assert!(shipped.contains("boot_grace_secs"));
        assert!(shipped.contains("backoff_base_secs"));
        assert!(shipped.contains("backoff_max_secs"));
        assert!(shipped.contains("stable_after_secs"));
        assert!(shipped.contains("reboot_deadline_secs"));
        assert!(shipped.contains("interval_secs"));
        assert!(shipped.contains("timeout_secs"));
        assert!(shipped.contains("threshold"));
        assert!(shipped.contains("actions"));
    }
}
