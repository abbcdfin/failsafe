use std::path::PathBuf;
use std::process;
use std::time::{Duration, Instant};

use failsafe::detector;
use failsafe::monitor::{Monitor, OutcomeEvent};
use failsafe::types::{Action, Outcome, Policy};
use failsafe::{log_error, log_info, log_warn};

struct Args {
    detectors_dir: PathBuf,
    dry_run: bool,
    once: bool,
    policy: Policy,
}

fn print_usage() {
    eprintln!(
        "Usage: failsafe [--detectors-dir PATH] [--dry-run] [--once]\n\n\
         Options:\n  \
           --detectors-dir PATH   Directory containing detectors (default: /etc/failsafe/detectors.d)\n  \
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
    let mut dry_run = false;
    let mut once = false;
    let mut policy = Policy::default();

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

    Args {
        detectors_dir,
        dry_run,
        once,
        policy,
    }
}

fn main() {
    let args = parse_args();

    if args.once {
        run_once(args);
    } else {
        if !args.dry_run {
            log_error!("reboot is not implemented; run with --dry-run");
            process::exit(1);
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

    let mut monitor = Monitor::new(&detectors, Instant::now());

    loop {
        let now = Instant::now();
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
                                match trip.action {
                                    Action::Reboot => {
                                        log_warn!(
                                            "would reboot: detector {}, {}, {} consecutive failures",
                                            trip.detector,
                                            trip.cause,
                                            trip.consecutive_failures
                                        );
                                    }
                                }
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
