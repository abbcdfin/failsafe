use std::path::Path;
use std::time::{Duration, Instant};

use crate::action::{self, Rebooter};
use crate::guard::{self, GuardConfig, RebootReason, State, Verdict};
use crate::monitor::Trip;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TripOutcome {
    Refused { remaining: Duration },
    DryRunAllowed,
    DryRunRefused { remaining: Duration },
    Executed,
    StateWriteFailed,
    RebootFailed,
}

#[allow(clippy::too_many_arguments)]
pub fn handle_trip(
    trip: &Trip,
    uptime: Duration,
    dry_run: bool,
    guard_config: &GuardConfig,
    state_dir: &Path,
    state: &mut State,
    rebooter: &mut dyn Rebooter,
    force_next: &mut bool,
) -> TripOutcome {
    let verdict = guard_config.check(state.streak, uptime);

    if dry_run {
        match verdict {
            Verdict::Allowed => {
                log_warn!(
                    "would reboot: detector {}, {}, {} consecutive failures",
                    trip.detector,
                    trip.cause,
                    trip.consecutive_failures
                );
                TripOutcome::DryRunAllowed
            }
            Verdict::Refused { remaining } => {
                log_warn!(
                    "would be refused: detector {}, {}, {} consecutive failures ({} s of backoff remaining)",
                    trip.detector,
                    trip.cause,
                    trip.consecutive_failures,
                    remaining.as_secs()
                );
                TripOutcome::DryRunRefused { remaining }
            }
        }
    } else {
        match verdict {
            Verdict::Refused { remaining } => {
                log_warn!(
                    "reboot refused: detector {}, {}, {} consecutive failures ({} s of backoff remaining)",
                    trip.detector,
                    trip.cause,
                    trip.consecutive_failures,
                    remaining.as_secs()
                );
                TripOutcome::Refused { remaining }
            }
            Verdict::Allowed => {
                log_warn!(
                    "rebooting: detector {}, {}, {} consecutive failures",
                    trip.detector,
                    trip.cause,
                    trip.consecutive_failures
                );

                let mut next_state = state.clone();
                next_state.streak = state.streak.saturating_add(1);
                next_state.last_reason = Some(RebootReason {
                    detector: trip.detector.clone(),
                    cause: trip.cause.clone(),
                    consecutive_failures: trip.consecutive_failures,
                    boot_id: action::read_boot_id(),
                });

                if let Err(e) = guard::save_state(state_dir, &next_state) {
                    log_error!("failed to write state file: {}; reboot refused", e);
                    return TripOutcome::StateWriteFailed;
                }

                *state = next_state;

                if let Err(e) = action::execute_reboot(
                    rebooter,
                    guard_config.reboot_deadline,
                    *force_next,
                ) {
                    log_error!(
                        "reboot failed: {}; attempt not counted, returning to monitoring",
                        e
                    );
                    state.streak = state.streak.saturating_sub(1);
                    state.last_reason = None;
                    let _ = guard::save_state(state_dir, state);
                    *force_next = true;
                    return TripOutcome::RebootFailed;
                }

                *force_next = false;
                TripOutcome::Executed
            }
        }
    }
}

pub fn process_startup_state(
    state: &mut State,
    current_boot_id: Option<&str>,
    state_dir: &Path,
    dry_run: bool,
) -> bool {
    let mut force_next = false;
    if let Some(ref reason) = state.last_reason {
        if let (Some(stored_id), Some(curr_id)) = (reason.boot_id.as_deref(), current_boot_id)
            && stored_id == curr_id
        {
            log_error!(
                "previous reboot attempt failed: system was restarted without reboot (boot ID unchanged)"
            );
            state.streak = state.streak.saturating_sub(1);
            state.last_reason = None;
            force_next = true;
            if !dry_run {
                let _ = guard::save_state(state_dir, state);
            }
            return force_next;
        }

        log_warn!(
            "previous reboot was initiated by failsafe: detector {}, {}, streak {}",
            reason.detector,
            reason.cause,
            state.streak
        );
    }
    force_next
}

pub fn check_stable(
    now: Instant,
    stable_since: Option<Instant>,
    guard_config: &GuardConfig,
    state_dir: &Path,
    state: &mut State,
    dry_run: bool,
) -> bool {
    if state.streak > 0
        && let Some(since) = stable_since
        && now.duration_since(since) >= guard_config.stable_after
    {
        log_info!("system is stable; clearing streak (was {})", state.streak);
        state.streak = 0;
        state.last_reason = None;
        if !dry_run
            && let Err(e) = guard::save_state(state_dir, state)
        {
            log_error!("failed to clear state file: {}", e);
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Action, Cause};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;

    struct FakeChild;
    impl action::RebootChild for FakeChild {
        fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, std::io::Error> {
            Ok(Some(ExitStatusExt::from_raw(0)))
        }
        fn kill(&mut self) -> Result<(), std::io::Error> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeRebooter {
        calls: Vec<String>,
        force_error: bool,
    }

    impl Rebooter for FakeRebooter {
        fn start_sync(&mut self) -> std::sync::mpsc::Receiver<()> {
            self.calls.push("sync".to_string());
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(());
            rx
        }

        fn request_reboot(&mut self) -> Result<Box<dyn action::RebootChild>, std::io::Error> {
            self.calls.push("request_reboot".to_string());
            Ok(Box::new(FakeChild))
        }

        fn force_reboot(&mut self) -> Result<(), std::io::Error> {
            self.calls.push("force_reboot".to_string());
            if self.force_error {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "force reboot failed",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn make_test_trip() -> Trip {
        Trip {
            detector: "lte-link".to_string(),
            cause: Cause::Exit(1),
            consecutive_failures: 3,
            action: Action::Reboot,
        }
    }

    fn temp_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("failsafe_orch_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_trip_refused_in_grace() {
        let dir = temp_test_dir("refused_grace");
        let mut state = State::default();
        let guard_config = GuardConfig {
            reboot_deadline: Duration::ZERO,
            ..Default::default()
        };
        let mut fake = FakeRebooter::default();
        let trip = make_test_trip();
        let mut force_next = false;

        // Uptime 100 s is within 300 s boot grace
        let outcome = handle_trip(
            &trip,
            Duration::from_secs(100),
            false,
            &guard_config,
            &dir,
            &mut state,
            &mut fake,
            &mut force_next,
        );

        assert_eq!(
            outcome,
            TripOutcome::Refused {
                remaining: Duration::from_secs(200)
            }
        );
        assert_eq!(state.streak, 0);
        assert!(fake.calls.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_trip_allowed_after_grace() {
        let dir = temp_test_dir("allowed_after_grace");
        let mut state = State::default();
        let guard_config = GuardConfig {
            reboot_deadline: Duration::ZERO,
            ..Default::default()
        };
        let mut fake = FakeRebooter::default();
        let trip = make_test_trip();
        let mut force_next = false;

        // Uptime 300 s satisfies boot grace
        let outcome = handle_trip(
            &trip,
            Duration::from_secs(300),
            false,
            &guard_config,
            &dir,
            &mut state,
            &mut fake,
            &mut force_next,
        );

        assert_eq!(outcome, TripOutcome::Executed);
        assert_eq!(state.streak, 1);
        assert!(!fake.calls.is_empty());

        let (loaded, err) = guard::load_state(&dir, 9);
        assert!(err.is_none());
        assert_eq!(loaded.streak, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_refused_again_at_streak_1_until_backoff_passes() {
        let dir = temp_test_dir("streak_backoff");
        let mut state = State {
            streak: 1,
            last_reason: None,
        };
        let guard_config = GuardConfig {
            reboot_deadline: Duration::ZERO,
            ..Default::default()
        };
        let mut fake = FakeRebooter::default();
        let trip = make_test_trip();
        let mut force_next = false;

        // For streak 1, required uptime is 300 + 600 = 900 s.
        // At 850 s, should be refused:
        let outcome = handle_trip(
            &trip,
            Duration::from_secs(850),
            false,
            &guard_config,
            &dir,
            &mut state,
            &mut fake,
            &mut force_next,
        );
        assert_eq!(
            outcome,
            TripOutcome::Refused {
                remaining: Duration::from_secs(50)
            }
        );
        assert_eq!(state.streak, 1);
        assert!(fake.calls.is_empty());

        // At 900 s, allowed:
        let outcome = handle_trip(
            &trip,
            Duration::from_secs(900),
            false,
            &guard_config,
            &dir,
            &mut state,
            &mut fake,
            &mut force_next,
        );
        assert_eq!(outcome, TripOutcome::Executed);
        assert_eq!(state.streak, 2);
        assert!(!fake.calls.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_state_write_fails_fake_never_called() {
        let dir = temp_test_dir("write_fails");
        // Make directory read-only so write fails
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();

        let mut state = State::default();
        let guard_config = GuardConfig {
            reboot_deadline: Duration::ZERO,
            ..Default::default()
        };
        let mut fake = FakeRebooter::default();
        let trip = make_test_trip();
        let mut force_next = false;

        let outcome = handle_trip(
            &trip,
            Duration::from_secs(300),
            false,
            &guard_config,
            &dir,
            &mut state,
            &mut fake,
            &mut force_next,
        );

        assert_eq!(outcome, TripOutcome::StateWriteFailed);
        assert_eq!(state.streak, 0);
        assert!(fake.calls.is_empty());

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_dry_run_state_file_unchanged() {
        let dir = temp_test_dir("dry_run_unchanged");
        let initial_state = State {
            streak: 0,
            last_reason: None,
        };
        guard::save_state(&dir, &initial_state).unwrap();

        let mut state = initial_state.clone();
        let guard_config = GuardConfig {
            reboot_deadline: Duration::ZERO,
            ..Default::default()
        };
        let mut fake = FakeRebooter::default();
        let trip = make_test_trip();
        let mut force_next = false;

        let outcome = handle_trip(
            &trip,
            Duration::from_secs(300),
            true, // dry_run
            &guard_config,
            &dir,
            &mut state,
            &mut fake,
            &mut force_next,
        );

        assert_eq!(outcome, TripOutcome::DryRunAllowed);
        assert_eq!(state.streak, 0);
        assert!(fake.calls.is_empty());

        let (loaded, err) = guard::load_state(&dir, 9);
        assert!(err.is_none());
        assert_eq!(loaded, initial_state);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_force_fails_leaves_streak_unchanged() {
        let dir = temp_test_dir("force_fails");
        let mut state = State::default();
        let guard_config = GuardConfig {
            reboot_deadline: Duration::ZERO,
            ..Default::default()
        };
        let mut fake = FakeRebooter {
            force_error: true,
            ..Default::default()
        };
        let trip = make_test_trip();
        let mut force_next = false;

        let outcome = handle_trip(
            &trip,
            Duration::from_secs(300),
            false,
            &guard_config,
            &dir,
            &mut state,
            &mut fake,
            &mut force_next,
        );

        assert_eq!(outcome, TripOutcome::RebootFailed);
        assert_eq!(state.streak, 0);
        assert_eq!(state.last_reason, None);
        assert!(force_next);

        let (loaded, err) = guard::load_state(&dir, 9);
        assert!(err.is_none());
        assert_eq!(loaded.streak, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_process_startup_state_same_boot_id() {
        let dir = temp_test_dir("startup_same_boot");
        let mut state = State {
            streak: 2,
            last_reason: Some(RebootReason {
                detector: "lte-link".to_string(),
                cause: Cause::Exit(1),
                consecutive_failures: 3,
                boot_id: Some("boot-uuid-1".to_string()),
            }),
        };
        guard::save_state(&dir, &state).unwrap();

        let force_next = process_startup_state(&mut state, Some("boot-uuid-1"), &dir, false);
        assert!(force_next);
        assert_eq!(state.streak, 1);
        assert_eq!(state.last_reason, None);

        let (loaded, _) = guard::load_state(&dir, 9);
        assert_eq!(loaded.streak, 1);
        assert_eq!(loaded.last_reason, None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_process_startup_state_different_boot_id() {
        let dir = temp_test_dir("startup_diff_boot");
        let mut state = State {
            streak: 2,
            last_reason: Some(RebootReason {
                detector: "lte-link".to_string(),
                cause: Cause::Exit(1),
                consecutive_failures: 3,
                boot_id: Some("boot-uuid-1".to_string()),
            }),
        };

        let force_next = process_startup_state(&mut state, Some("boot-uuid-2"), &dir, false);
        assert!(!force_next);
        assert_eq!(state.streak, 2);
        assert!(state.last_reason.is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_stable_clears_streak_exactly_once() {
        let dir = temp_test_dir("stable_clears");
        let mut state = State {
            streak: 2,
            last_reason: Some(RebootReason {
                detector: "lte-link".to_string(),
                cause: Cause::Exit(1),
                consecutive_failures: 3,
                boot_id: None,
            }),
        };
        guard::save_state(&dir, &state).unwrap();

        let guard_config = GuardConfig::default();
        let t0 = Instant::now();

        // Not stable yet (stable_after is 1800 s)
        let cleared = check_stable(
            t0 + Duration::from_secs(500),
            Some(t0),
            &guard_config,
            &dir,
            &mut state,
            false,
        );
        assert!(!cleared);
        assert_eq!(state.streak, 2);

        // After stable_after: clears streak
        let cleared = check_stable(
            t0 + Duration::from_secs(1800),
            Some(t0),
            &guard_config,
            &dir,
            &mut state,
            false,
        );
        assert!(cleared);
        assert_eq!(state.streak, 0);
        assert_eq!(state.last_reason, None);

        let (loaded, _) = guard::load_state(&dir, 9);
        assert_eq!(loaded.streak, 0);
        assert_eq!(loaded.last_reason, None);

        // Subsequent call: streak is already 0, does not write again
        let cleared = check_stable(
            t0 + Duration::from_secs(1900),
            Some(t0),
            &guard_config,
            &dir,
            &mut state,
            false,
        );
        assert!(!cleared);
        let _ = fs::remove_dir_all(dir);
    }
}
