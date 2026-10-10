use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{Duration, Instant};

use failsafe::action::{self, SystemRebooter};
use failsafe::detector;
use failsafe::guard::{self, GuardConfig};
use failsafe::monitor::{Monitor, OutcomeEvent};
use failsafe::orchestration;
use failsafe::types::{Outcome, Policy};
use failsafe::{log_error, log_info, log_warn};

struct Args {
    detectors_dir: PathBuf,
    state_dir: PathBuf,
    dry_run: bool,
    once: bool,
    policy: Policy,
    test_uptime: Option<Duration>,
}

fn print_usage() {
    eprintln!(
        "Usage: failsafe [--detectors-dir PATH] [--state-dir PATH] [--dry-run] [--once]\n\n\
         Options:\n  \
           --detectors-dir PATH   Directory containing detectors (default: /etc/failsafe/detectors.d)\n  \
           --state-dir PATH       Directory for state persistence (default: /var/lib/failsafe)\n  \
           --dry-run              Do not reboot; log what would have been done\n  \
           --once                 Run each detector once, report outcomes, and exit\n  \
           -h, --help             Show this help message"
    );
}

fn parse_test_interval(val: &str, policy: &mut Policy) {
    match val.parse::<u64>() {
        Ok(ms) if ms > 0 => {
            policy.interval = Duration::from_millis(ms);
        }
        _ => {
            log_error!("invalid value for --test-interval-ms: must be a positive integer");
            process::exit(1);
        }
    }
}

fn parse_test_timeout(val: &str, policy: &mut Policy) {
    match val.parse::<u64>() {
        Ok(ms) if ms > 0 => {
            policy.timeout = Duration::from_millis(ms);
        }
        _ => {
            log_error!("invalid value for --test-timeout-ms: must be a positive integer");
            process::exit(1);
        }
    }
}

fn parse_test_threshold(val: &str, policy: &mut Policy) {
    match val.parse::<u32>() {
        Ok(threshold) if threshold > 0 => {
            policy.threshold = threshold;
        }
        _ => {
            log_error!("invalid value for --test-threshold: must be a positive integer");
            process::exit(1);
        }
    }
}

fn parse_args() -> Args {
    let mut detectors_dir = PathBuf::from("/etc/failsafe/detectors.d");
    let mut state_dir = PathBuf::from("/var/lib/failsafe");
    let mut dry_run = false;
    let mut once = false;
    let mut policy = Policy::default();
    let mut test_uptime = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--once" {
            once = true;
        } else if arg == "-h" || arg == "--help" {
            print_usage();
            process::exit(0);
        } else if arg == "--detectors-dir" {
            i += 1;
            if i >= args.len() {
                log_error!("missing argument for --detectors-dir");
                process::exit(1);
            }
            detectors_dir = PathBuf::from(&args[i]);
        } else if let Some(val) = arg.strip_prefix("--detectors-dir=") {
            detectors_dir = PathBuf::from(val);
        } else if arg == "--state-dir" {
            i += 1;
            if i >= args.len() {
                log_error!("missing argument for --state-dir");
                process::exit(1);
            }
            state_dir = PathBuf::from(&args[i]);
        } else if let Some(val) = arg.strip_prefix("--state-dir=") {
            state_dir = PathBuf::from(val);
        } else if arg == "--test-interval-ms" {
            i += 1;
            if i >= args.len() {
                log_error!("missing argument for --test-interval-ms");
                process::exit(1);
            }
            parse_test_interval(&args[i], &mut policy);
        } else if let Some(val) = arg.strip_prefix("--test-interval-ms=") {
            parse_test_interval(val, &mut policy);
        } else if arg == "--test-timeout-ms" {
            i += 1;
            if i >= args.len() {
                log_error!("missing argument for --test-timeout-ms");
                process::exit(1);
            }
            parse_test_timeout(&args[i], &mut policy);
        } else if let Some(val) = arg.strip_prefix("--test-timeout-ms=") {
            parse_test_timeout(val, &mut policy);
        } else if arg == "--test-threshold" {
            i += 1;
            if i >= args.len() {
                log_error!("missing argument for --test-threshold");
                process::exit(1);
            }
            parse_test_threshold(&args[i], &mut policy);
        } else if let Some(val) = arg.strip_prefix("--test-threshold=") {
            parse_test_threshold(val, &mut policy);
        } else if arg == "--test-uptime-secs" {
            i += 1;
            if i >= args.len() {
                log_error!("missing argument for --test-uptime-secs");
                process::exit(1);
            }
            match args[i].parse::<u64>() {
                Ok(secs) => test_uptime = Some(Duration::from_secs(secs)),
                _ => {
                    log_error!("invalid value for --test-uptime-secs: must be a positive integer");
                    process::exit(1);
                }
            }
        } else if let Some(val) = arg.strip_prefix("--test-uptime-secs=") {
            match val.parse::<u64>() {
                Ok(secs) => test_uptime = Some(Duration::from_secs(secs)),
                _ => {
                    log_error!("invalid value for --test-uptime-secs: must be a positive integer");
                    process::exit(1);
                }
            }
        } else {
            log_error!("unknown argument: {arg}");
            process::exit(1);
        }
        i += 1;
    }

    if let Err(err) = policy.validate() {
        log_error!("invalid policy: {err}");
        process::exit(1);
    }

    if test_uptime.is_some() && !dry_run {
        log_error!("--test-uptime-secs is only permitted with --dry-run");
        process::exit(1);
    }

    Args {
        detectors_dir,
        state_dir,
        dry_run,
        once,
        policy,
        test_uptime,
    }
}

fn main() {
    let args = parse_args();

    if args.once {
        run_once(args);
    } else {
        if !args.dry_run {
            let euid = unsafe { libc::geteuid() };
            if euid != 0 {
                log_error!(
                    "failsafe must run as root in real mode (effective uid is {euid})"
                );
                process::exit(1);
            }
        }
        run_daemon(args);
    }
}

fn run_once(args: Args) {
    let detectors = match detector::discover(&args.detectors_dir, &args.policy) {
        Ok(dets) => dets,
        Err(err) => {
            log_error!(
                "cannot read detectors directory {}: {}",
                args.detectors_dir.display(),
                err
            );
            process::exit(1);
        }
    };

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

fn run_daemon(args: Args) {
    let detectors = match detector::discover(&args.detectors_dir, &args.policy) {
        Ok(dets) => dets,
        Err(err) => {
            log_error!(
                "cannot read detectors directory {}: {}",
                args.detectors_dir.display(),
                err
            );
            process::exit(1);
        }
    };

    log_info!("watching detectors in {}", args.detectors_dir.display());
    if detectors.is_empty() {
        log_warn!("no detectors found in {}", args.detectors_dir.display());
    } else {
        for d in &detectors {
            log_info!("found detector: {}", d.name);
        }
    }

    let guard_config = GuardConfig::default();

    if !args.dry_run {
        let _ = fs::create_dir_all(&args.state_dir);
        let probe = args
            .state_dir
            .join(format!(".probe.{}", std::process::id()));
        if let Err(e) = fs::write(&probe, b"") {
            log_error!(
                "state directory {} is not writable: {}; reboots will be refused",
                args.state_dir.display(),
                e
            );
        } else {
            let _ = fs::remove_file(probe);
        }
    }

    let (mut state, load_err) =
        guard::load_state(&args.state_dir, guard_config.worst_case_streak());
    if let Some(err) = load_err {
        log_error!("{err}; streak set to {}", state.streak);
    }

    let current_boot_id = action::read_boot_id();
    let mut force_next = orchestration::process_startup_state(
        &mut state,
        current_boot_id.as_deref(),
        &args.state_dir,
        args.dry_run,
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
            &guard_config,
            &args.state_dir,
            &mut state,
            args.dry_run,
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
                                let uptime = args
                                    .test_uptime
                                    .unwrap_or_else(action::read_uptime);
                                orchestration::handle_trip(
                                    &trip,
                                    uptime,
                                    args.dry_run,
                                    &guard_config,
                                    &args.state_dir,
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
