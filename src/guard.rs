use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::Cause;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardConfig {
    pub boot_grace: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub stable_after: Duration,
    pub reboot_deadline: Duration,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            boot_grace: Duration::from_secs(300),
            backoff_base: Duration::from_secs(600),
            backoff_max: Duration::from_secs(86400),
            stable_after: Duration::from_secs(1800),
            reboot_deadline: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Refused { remaining: Duration },
}

impl GuardConfig {
    pub fn required_uptime(&self, streak: u32) -> Duration {
        if streak == 0 {
            self.boot_grace
        } else {
            let exponent = streak - 1;
            let backoff_secs = if exponent >= 30 {
                self.backoff_max.as_secs()
            } else {
                let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
                let product = self.backoff_base.as_secs().saturating_mul(multiplier);
                std::cmp::min(product, self.backoff_max.as_secs())
            };
            self.boot_grace
                .saturating_add(Duration::from_secs(backoff_secs))
        }
    }

    pub fn check(&self, streak: u32, uptime: Duration) -> Verdict {
        let req = self.required_uptime(streak);
        if uptime >= req {
            Verdict::Allowed
        } else {
            Verdict::Refused {
                remaining: req - uptime,
            }
        }
    }

    pub fn worst_case_streak(&self) -> u32 {
        let max_secs = self.backoff_max.as_secs();
        let base_secs = self.backoff_base.as_secs().max(1);
        let mut streak = 1u32;
        while streak < 32 {
            let multiplier = 1u64.checked_shl(streak - 1).unwrap_or(u64::MAX);
            if base_secs.saturating_mul(multiplier) >= max_secs {
                return streak;
            }
            streak += 1;
        }
        streak
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebootReason {
    pub detector: String,
    pub cause: Cause,
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct State {
    pub streak: u32,
    pub last_reason: Option<RebootReason>,
}

pub fn load_state(state_dir: &Path, worst_case_streak: u32) -> (State, Option<String>) {
    let path = state_dir.join("state.toml");

    match fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<State>(&content) {
            Ok(state) => (state, None),
            Err(e) => (
                State {
                    streak: worst_case_streak,
                    last_reason: None,
                },
                Some(format!("corrupt state file {}: {}", path.display(), e)),
            ),
        },
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                (
                    State {
                        streak: 0,
                        last_reason: None,
                    },
                    None,
                )
            } else {
                (
                    State {
                        streak: worst_case_streak,
                        last_reason: None,
                    },
                    Some(format!("unreadable state file {}: {}", path.display(), e)),
                )
            }
        }
    }
}

pub fn save_state(state_dir: &Path, state: &State) -> Result<(), std::io::Error> {
    let content = toml::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let temp_path = state_dir.join(format!(".state.tmp.{}", std::process::id()));
    let target_path = state_dir.join("state.toml");

    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }

    fs::rename(&temp_path, &target_path)?;

    let dir_file = fs::File::open(state_dir)?;
    dir_file.sync_all()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_required_uptime_sequence() {
        let config = GuardConfig::default();
        // 5, 15, 25, 45, 85 min
        assert_eq!(config.required_uptime(0), Duration::from_secs(5 * 60));
        assert_eq!(config.required_uptime(1), Duration::from_secs(15 * 60));
        assert_eq!(config.required_uptime(2), Duration::from_secs(25 * 60));
        assert_eq!(config.required_uptime(3), Duration::from_secs(45 * 60));
        assert_eq!(config.required_uptime(4), Duration::from_secs(85 * 60));
    }

    #[test]
    fn test_required_uptime_ceiling_and_overflow() {
        let config = GuardConfig::default();
        let ceiling = config.boot_grace + config.backoff_max; // 300 + 86400 = 86700 s
        assert_eq!(config.required_uptime(9), ceiling);
        assert_eq!(config.required_uptime(10), ceiling);
        assert_eq!(config.required_uptime(31), ceiling);
        assert_eq!(config.required_uptime(u32::MAX), ceiling);
    }

    #[test]
    fn test_check_verdict() {
        let config = GuardConfig::default();
        // streak 0: required uptime is 300 s
        assert_eq!(
            config.check(0, Duration::from_secs(200)),
            Verdict::Refused {
                remaining: Duration::from_secs(100)
            }
        );
        assert_eq!(config.check(0, Duration::from_secs(300)), Verdict::Allowed);
        assert_eq!(config.check(0, Duration::from_secs(400)), Verdict::Allowed);

        // streak 1: required uptime is 900 s
        assert_eq!(
            config.check(1, Duration::from_secs(850)),
            Verdict::Refused {
                remaining: Duration::from_secs(50)
            }
        );
        assert_eq!(config.check(1, Duration::from_secs(900)), Verdict::Allowed);
    }

    #[test]
    fn test_worst_case_streak() {
        let config = GuardConfig::default();
        let worst = config.worst_case_streak();
        assert_eq!(worst, 9);
        assert_eq!(
            config.required_uptime(worst),
            config.boot_grace + config.backoff_max
        );
    }

    fn temp_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("failsafe_guard_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_state_load_missing() {
        let dir = temp_test_dir("missing");
        let (state, err) = load_state(&dir, 9);
        assert_eq!(state.streak, 0);
        assert_eq!(state.last_reason, None);
        assert!(err.is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_state_save_and_load_round_trip() {
        let dir = temp_test_dir("roundtrip");
        let state = State {
            streak: 3,
            last_reason: Some(RebootReason {
                detector: "lte-link".to_string(),
                cause: Cause::Exit(1),
                consecutive_failures: 4,
                boot_id: None,
            }),
        };

        save_state(&dir, &state).unwrap();

        let (loaded, err) = load_state(&dir, 9);
        assert!(err.is_none());
        assert_eq!(loaded, state);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_state_load_corrupt() {
        let dir = temp_test_dir("corrupt");
        fs::write(dir.join("state.toml"), "invalid toml content = {{{{").unwrap();

        let (state, err) = load_state(&dir, 9);
        assert_eq!(state.streak, 9);
        assert_eq!(state.last_reason, None);
        assert!(err.is_some());
        assert!(err.unwrap().contains("corrupt state file"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_state_save_read_only_dir() {
        let dir = temp_test_dir("readonly");
        // Make directory read-only
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();

        let state = State {
            streak: 1,
            last_reason: None,
        };
        let res = save_state(&dir, &state);
        assert!(res.is_err());
        // Restore permissions to clean up
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_state_load_inaccessible() {
        let dir = temp_test_dir("inaccessible");
        let file = dir.join("state.toml");
        fs::write(&file, "streak = 5\n").unwrap();
        // Remove read permissions
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let (state, err) = load_state(&dir, 9);
        assert_eq!(state.streak, 9);
        assert!(err.is_some());
        assert!(err.unwrap().contains("unreadable state file"));

        // Restore permissions to clean up
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let _ = fs::remove_dir_all(dir);
    }
}

