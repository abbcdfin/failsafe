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
    pub policy: Policy,
    pub next_run: Instant,
}

pub struct Monitor {
    states: HashMap<String, DetectorState>,
    order: Vec<String>,
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
                    policy: d.policy.clone(),
                    next_run: start_time,
                },
            );
            order.push(d.name.clone());
        }

        Self { states, order }
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
                let was_failing = state.consecutive_failures > 0;
                state.consecutive_failures = 0;
                if was_failing {
                    Some(OutcomeEvent::Recovered)
                } else {
                    Some(OutcomeEvent::Healthy)
                }
            }
            Outcome::Unknown => Some(OutcomeEvent::Unknown),
            Outcome::Broken(_) => Some(OutcomeEvent::Broken),
            Outcome::Unhealthy(cause) => {
                state.consecutive_failures += 1;
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

        let event = monitor.record_outcome("d1", Outcome::Healthy, start);
        assert_eq!(event, Some(OutcomeEvent::Recovered));
        assert_eq!(monitor.consecutive_failures("d1"), Some(0));

        let event = monitor.record_outcome("d1", Outcome::Healthy, start);
        assert_eq!(event, Some(OutcomeEvent::Healthy));
        assert_eq!(monitor.consecutive_failures("d1"), Some(0));
    }

    #[test]
    fn test_unknown_and_broken_leave_counter_unchanged() {
        let start = Instant::now();
        let detectors = vec![make_detector("d1", 3, 60)];
        let mut monitor = Monitor::new(&detectors, start);

        monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), start);
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));

        let event = monitor.record_outcome("d1", Outcome::Unknown, start);
        assert_eq!(event, Some(OutcomeEvent::Unknown));
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));

        let event = monitor.record_outcome("d1", Outcome::Broken("error".into()), start);
        assert_eq!(event, Some(OutcomeEvent::Broken));
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));
    }

    #[test]
    fn test_trip_at_threshold_and_reset_on_action_attempted() {
        let start = Instant::now();
        let detectors = vec![make_detector("d1", 2, 60)];
        let mut monitor = Monitor::new(&detectors, start);

        let event1 = monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), start);
        assert_eq!(
            event1,
            Some(OutcomeEvent::Unhealthy {
                cause: Cause::Exit(1),
                consecutive_failures: 1,
                threshold: 2,
                trip: None,
            })
        );
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));

        let event2 = monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Timeout), start);
        let trip = Trip {
            detector: "d1".to_string(),
            cause: Cause::Timeout,
            consecutive_failures: 2,
            action: Action::Reboot,
        };
        assert_eq!(
            event2,
            Some(OutcomeEvent::Unhealthy {
                cause: Cause::Timeout,
                consecutive_failures: 2,
                threshold: 2,
                trip: Some(trip),
            })
        );
        assert_eq!(monitor.consecutive_failures("d1"), Some(2));

        // After action is attempted, counter resets to 0
        monitor.action_attempted("d1");
        assert_eq!(monitor.consecutive_failures("d1"), Some(0));

        // Persisting failure requires threshold runs again to re-trip
        let event3 = monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), start);
        assert_eq!(
            event3,
            Some(OutcomeEvent::Unhealthy {
                cause: Cause::Exit(1),
                consecutive_failures: 1,
                threshold: 2,
                trip: None,
            })
        );
        assert_eq!(monitor.consecutive_failures("d1"), Some(1));

        let event4 = monitor.record_outcome("d1", Outcome::Unhealthy(Cause::Exit(1)), start);
        match event4 {
            Some(OutcomeEvent::Unhealthy { trip: Some(_), .. }) => {}
            other => panic!("expected trip, got {:?}", other),
        }
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
