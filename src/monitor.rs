use std::collections::HashMap;
use std::time::Instant;

use crate::types::{Action, Cause, Detector, Outcome, Policy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trip {
    pub detector: String,
    pub cause: Cause,
    pub consecutive_failures: u32,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeEvent {
    Healthy,
    Recovered,
    Unknown,
    Broken,
    Unhealthy {
        cause: Cause,
        consecutive_failures: u32,
        threshold: u32,
        trip: Option<Trip>,
    },
}

#[derive(Debug, Clone)]
pub struct DetectorState {
    pub name: String,
    pub consecutive_failures: u32,
    pub failing: bool,
    pub policy: Policy,
    pub next_run: Instant,
}

pub struct Monitor {
    states: HashMap<String, DetectorState>,
    order: Vec<String>,
    stable_since: Option<Instant>,
}

impl Monitor {
    pub fn new(detectors: &[Detector], start_time: Instant) -> Self {
        let mut states = HashMap::new();
        let mut order = Vec::new();

        for d in detectors {
            assert!(
                !d.policy.actions.is_empty(),
                "detector {} has empty actions",
                d.name
            );
            states.insert(
                d.name.clone(),
                DetectorState {
                    name: d.name.clone(),
                    consecutive_failures: 0,
                    failing: false,
                    policy: d.policy.clone(),
                    next_run: start_time,
                },
            );
            order.push(d.name.clone());
        }

        Self {
            states,
            order,
            stable_since: Some(start_time),
        }
    }

    pub fn next_due_detector(&self, now: Instant) -> Option<String> {
        for name in &self.order {
            if self.states.get(name).is_some_and(|s| s.next_run <= now) {
                return Some(name.clone());
            }
        }
        None
    }

    pub fn next_due_at(&self) -> Option<Instant> {
        self.states.values().map(|s| s.next_run).min()
    }

    pub fn stable_since(&self) -> Option<Instant> {
        self.stable_since
    }

    pub fn is_failing(&self, name: &str) -> Option<bool> {
        self.states.get(name).map(|s| s.failing)
    }

    pub fn record_outcome(
        &mut self,
        name: &str,
        outcome: Outcome,
        now: Instant,
    ) -> Option<OutcomeEvent> {
        let state = self.states.get_mut(name)?;
        state.next_run = now + state.policy.interval;

        match outcome {
            Outcome::Healthy => {
                let was_failing = state.failing;
                state.consecutive_failures = 0;
                state.failing = false;

                if was_failing {
                    if self.states.values().all(|s| !s.failing) {
                        self.stable_since = Some(now);
                    }
                    Some(OutcomeEvent::Recovered)
                } else {
                    Some(OutcomeEvent::Healthy)
                }
            }
            Outcome::Unknown => Some(OutcomeEvent::Unknown),
            Outcome::Broken(_) => Some(OutcomeEvent::Broken),
            Outcome::Unhealthy(cause) => {
                state.consecutive_failures += 1;
                state.failing = true;
                self.stable_since = None;

                let trip = if state.consecutive_failures >= state.policy.threshold {
                    let action = state
                        .policy
                        .actions
                        .first()
                        .expect("policy actions must not be empty")
                        .clone();
                    Some(Trip {
                        detector: name.to_string(),
                        cause: cause.clone(),
                        consecutive_failures: state.consecutive_failures,
                        action,
                    })
                } else {
                    None
                };
                Some(OutcomeEvent::Unhealthy {
                    cause,
                    consecutive_failures: state.consecutive_failures,
                    threshold: state.policy.threshold,
                    trip,
                })
            }
        }
    }

    pub fn action_attempted(&mut self, name: &str) {
        if let Some(state) = self.states.get_mut(name) {
            state.consecutive_failures = 0;
        }
    }

    pub fn consecutive_failures(&self, name: &str) -> Option<u32> {
        self.states.get(name).map(|s| s.consecutive_failures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn make_detector(name: &str, threshold: u32, interval_secs: u64) -> Detector {
        Detector {
            name: name.to_string(),
            path: PathBuf::from(format!("/fake/{}", name)),
            policy: Policy {
                interval: Duration::from_secs(interval_secs),
                timeout: Duration::from_secs(5),
                threshold,
                actions: vec![Action::Reboot],
            },
        }
    }

    #[test]
    fn test_healthy_resets_counter_and_reports_recovered() {
        let start = Instant::now();
        let detectors = vec![make_detector("d1", 3, 60)];
        let mut monitor = Monitor::new(&detectors, start);
        assert_eq!(monitor.stable_since(), Some(start));

        let event = monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), start);
        assert_eq!(
            event,
            Some(OutcomeEvent::Unhealthy {
                cause: Cause::Exit(1),
                consecutive_failures: 1,
                threshold: 3,
                trip: None,
            })
        );
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));
        assert_eq!(monitor.is_failing("d1"), Some(true));
        assert_eq!(monitor.stable_since(), None);

        let t1 = start + Duration::from_secs(60);
        let event = monitor.record_outcome("d1", Outcome::Healthy, t1);
        assert_eq!(event, Some(OutcomeEvent::Recovered));
        assert_eq!(monitor.consecutive_failures("d1"), Some(0));
        assert_eq!(monitor.is_failing("d1"), Some(false));
        assert_eq!(monitor.stable_since(), Some(t1));

        let t2 = t1 + Duration::from_secs(60);
        let event = monitor.record_outcome("d1", Outcome::Healthy, t2);
        assert_eq!(event, Some(OutcomeEvent::Healthy));
        assert_eq!(monitor.consecutive_failures("d1"), Some(0));
        assert_eq!(monitor.stable_since(), Some(t1)); // not bumped forward
    }

    #[test]
    fn test_trip_then_healthy_yields_recovered_and_trip_does_not_start_stable() {
        let start = Instant::now();
        let detectors = vec![make_detector("d1", 2, 60)];
        let mut monitor = Monitor::new(&detectors, start);

        let t1 = start;
        monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), t1);
        assert_eq!(monitor.stable_since(), None);

        let t2 = t1 + Duration::from_secs(60);
        let event = monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Timeout), t2);
        assert!(matches!(
            event,
            Some(OutcomeEvent::Unhealthy { trip: Some(_), .. })
        ));
        assert_eq!(monitor.consecutive_failures("d1"), Some(2));
        assert_eq!(monitor.is_failing("d1"), Some(true));

        // Attempt action (executed, dry-run, or refused)
        monitor.action_attempted("d1");
        assert_eq!(monitor.consecutive_failures("d1"), Some(0));
        assert_eq!(monitor.is_failing("d1"), Some(true));
        // Trip does NOT start the stable clock
        assert_eq!(monitor.stable_since(), None);

        // Another unhealthy run increments counter and stays failing
        let t3 = t2 + Duration::from_secs(60);
        monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), t3);
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));
        assert_eq!(monitor.is_failing("d1"), Some(true));
        assert_eq!(monitor.stable_since(), None);

        // Next run is healthy: transitions failing true -> false, emits Recovered, starts stable clock
        let t4 = t3 + Duration::from_secs(60);
        let event = monitor.record_outcome("d1", Outcome::Healthy, t4);
        assert_eq!(event, Some(OutcomeEvent::Recovered));
        assert_eq!(monitor.consecutive_failures("d1"), Some(0));
        assert_eq!(monitor.is_failing("d1"), Some(false));
        assert_eq!(monitor.stable_since(), Some(t4));
    }

    #[test]
    fn test_multiple_detectors_stability() {
        let start = Instant::now();
        let detectors = vec![make_detector("d1", 2, 60), make_detector("d2", 2, 60)];
        let mut monitor = Monitor::new(&detectors, start);
        assert_eq!(monitor.stable_since(), Some(start));

        // d1 fails -> system not stable
        monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), start);
        assert_eq!(monitor.stable_since(), None);

        // d2 healthy, but d1 still failing -> still not stable
        let t1 = start + Duration::from_secs(60);
        monitor.record_outcome("d2", Outcome::Healthy, t1);
        assert_eq!(monitor.stable_since(), None);

        // d1 recovers -> all healthy, system stable since t2
        let t2 = t1 + Duration::from_secs(60);
        monitor.record_outcome("d1", Outcome::Healthy, t2);
        assert_eq!(monitor.stable_since(), Some(t2));
    }

    #[test]
    fn test_unknown_and_broken_leave_counter_and_failing_unchanged() {
        let start = Instant::now();
        let detectors = vec![make_detector("d1", 3, 60)];
        let mut monitor = Monitor::new(&detectors, start);

        monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), start);
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));
        assert_eq!(monitor.is_failing("d1"), Some(true));

        let event = monitor.record_outcome("d1", Outcome::Unknown, start);
        assert_eq!(event, Some(OutcomeEvent::Unknown));
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));
        assert_eq!(monitor.is_failing("d1"), Some(true));

        let event = monitor.record_outcome("d1", Outcome::Broken("error".into()), start);
        assert_eq!(event, Some(OutcomeEvent::Broken));
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));
        assert_eq!(monitor.is_failing("d1"), Some(true));
    }

    #[test]
    fn test_scheduling_and_due() {
        let start = Instant::now();
        let detectors = vec![make_detector("d1", 3, 10), make_detector("d2", 3, 20)];
        let mut monitor = Monitor::new(&detectors, start);

        assert_eq!(monitor.next_due_detector(start), Some("d1".to_string()));

        monitor.record_outcome("d1", Outcome::Healthy, start);

        assert_eq!(monitor.next_due_detector(start), Some("d2".to_string()));

        monitor.record_outcome("d2", Outcome::Healthy, start);

        assert_eq!(monitor.next_due_detector(start), None);
        assert_eq!(monitor.next_due_at(), Some(start + Duration::from_secs(10)));

        let t10 = start + Duration::from_secs(10);
        assert_eq!(monitor.next_due_detector(t10), Some("d1".to_string()));
    }
}
