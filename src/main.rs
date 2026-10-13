use std::fs;
use std::process;
use std::time::{Duration, Instant};

use failsafe::action::{self, SystemRebooter};
use failsafe::config::{self, Config, LoadError};
use failsafe::detector;
use failsafe::guard;
use failsafe::monitor::{Monitor, OutcomeEvent};
use failsafe::orchestration;
use failsafe::types::Outcome;
use failsafe::{log_error, log_info, log_warn};

fn main() {
    let config = match config::load(std::env::args().skip(1)) {
        Ok(cfg) => cfg,
        Err(LoadError::Help) => {
            config::print_usage();
            process::exit(0);
        }
        Err(LoadError::Exit(msg)) => {
            log_error!("{msg}");
            process::exit(1);
        }
    };

    if config.once {
        run_once(&config);
    } else {
        if !config.dry_run {
            let euid = unsafe { libc::geteuid() };
            if euid != 0 {
                log_error!(
                    "failsafe must run as root in real mode (effective uid is {euid})"
                );
                process::exit(1);
            }
        }
        run_daemon(config);
    }
}

fn validate_unmatched_detectors(config: &Config, detectors: &[failsafe::types::Detector]) {
    for name in config.detectors.keys() {
        if !detectors.iter().any(|d| d.name == *name) {
            if let Some(ref path) = config.loaded_from {
                log_error!(
                    "config file {}: detector '{name}' has no matching file in {}",
                    path.display(),
                    config.detectors_dir.display()
                );
            } else {
                log_error!(
                    "detector '{name}' has no matching file in {}",
                    config.detectors_dir.display()
                );
            }
            process::exit(1);
        }
    }
}

fn run_once(config: &Config) {
    let detectors = match detector::discover(&config.detectors_dir, config) {
        Ok(dets) => dets,
        Err(err) => {
            log_error!(
                "cannot read detectors directory {}: {}",
                config.detectors_dir.display(),
                err
            );
            process::exit(1);
        }
    };

    validate_unmatched_detectors(config, &detectors);

    let mut failed = false;
    for d in &detectors {
        let outcome = detector::run(&d.path, d.policy.timeout);
        println!("{} {}", d.name, outcome);
        match outcome {
            Outcome::Unhealthy(_) | Outcome::Broken(_) => {
                failed = true;
            }
            Outcome::Healthy | Outcome::Unknown => {}
        }
    }

    if failed {
        process::exit(1);
    } else {
        process::exit(0);
    }
}

fn run_daemon(config: Config) {
    let detectors = match detector::discover(&config.detectors_dir, &config) {
        Ok(dets) => dets,
        Err(err) => {
            log_error!(
                "cannot read detectors directory {}: {}",
                config.detectors_dir.display(),
                err
            );
            process::exit(1);
        }
    };

    validate_unmatched_detectors(&config, &detectors);

    if let Some(ref path) = config.loaded_from {
        log_info!("using configuration from {}", path.display());
    } else {
        log_info!("using built-in defaults");
    }

    log_info!("watching detectors in {}", config.detectors_dir.display());
    if detectors.is_empty() {
        log_warn!("no detectors found in {}", config.detectors_dir.display());
    } else {
        for d in &detectors {
            log_info!("found detector: {}", d.name);
        }
    }

    for d in &detectors {
        if d.policy != config.defaults {
            let actions_str = d
                .policy
                .actions
                .iter()
                .map(|a| match a {
                    failsafe::types::Action::Reboot => "\"reboot\"",
                })
                .collect::<Vec<_>>()
                .join(", ");
            log_info!(
                "detector {} overrides defaults: interval_secs={}, timeout_secs={}, threshold={}, actions=[{actions_str}]",
                d.name,
                d.policy.interval.as_secs(),
                d.policy.timeout.as_secs(),
                d.policy.threshold,
            );
        }
    }

    if !config.dry_run {
        let _ = fs::create_dir_all(&config.state_dir);
        let probe = config
            .state_dir
            .join(format!(".probe.{}", std::process::id()));
        if let Err(e) = fs::write(&probe, b"") {
            log_error!(
                "state directory {} is not writable: {}; reboots will be refused",
                config.state_dir.display(),
                e
            );
        } else {
            let _ = fs::remove_file(probe);
        }
    }

    let (mut state, load_err) =
        guard::load_state(&config.state_dir, config.reboot.worst_case_streak());
    if let Some(err) = load_err {
        log_error!("{err}; streak set to {}", state.streak);
    }

    let current_boot_id = action::read_boot_id();
    let mut force_next = orchestration::process_startup_state(
        &mut state,
        current_boot_id.as_deref(),
        &config.state_dir,
        config.dry_run,
    );
    if state.streak > 0 && state.last_reason.is_none() {
        log_info!("current reboot streak is {}", state.streak);
    }

    let mut monitor = Monitor::new(&detectors, Instant::now());
    let mut rebooter = SystemRebooter;

    loop {
        let now = Instant::now();

        orchestration::check_stable(
            now,
            monitor.stable_since(),
            &config.reboot,
            &config.state_dir,
            &mut state,
            config.dry_run,
        );

        if let Some(name) = monitor.next_due_detector(now) {
            if let Some(d) = detectors.iter().find(|d| d.name == name) {
                let outcome = detector::run(&d.path, d.policy.timeout);
                if let Outcome::Broken(ref err) = outcome {
                    log_error!("detector {} is broken: {}", name, err);
                }
                if let Some(event) = monitor.record_outcome(&name, outcome, Instant::now()) {
                    match event {
                        OutcomeEvent::Recovered => {
                            log_info!("detector {} returned to healthy", name);
                        }
                        OutcomeEvent::Unhealthy {
                            cause,
                            consecutive_failures,
                            threshold,
                            trip,
                        } => {
                            log_warn!(
                                "detector {} unhealthy ({}), {}/{}",
                                name,
                                cause,
                                consecutive_failures,
                                threshold
                            );
                            if let Some(trip) = trip {
                                let uptime = action::read_uptime();
                                orchestration::handle_trip(
                                    &trip,
                                    uptime,
                                    config.dry_run,
                                    &config.reboot,
                                    &config.state_dir,
                                    &mut state,
                                    &mut rebooter,
                                    &mut force_next,
                                );
                                monitor.action_attempted(&trip.detector);
                            }
                        }
                        OutcomeEvent::Healthy | OutcomeEvent::Unknown | OutcomeEvent::Broken => {}
                    }
                }
            }
        } else {
            let sleep_duration = match monitor.next_due_at() {
                Some(next) => {
                    let now = Instant::now();
                    if next > now {
                        next - now
                    } else {
                        Duration::ZERO
                    }
                }
                None => Duration::from_secs(3600),
            };
            if !sleep_duration.is_zero() {
                std::thread::sleep(sleep_duration);
            }
        }
    }
}
