use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_test() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_failsafe"))
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("failsafe_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("failed to create temp dir");
        TempDir { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_script(dir: &Path, name: &str, content: &str, executable: bool) -> PathBuf {
    let file_path = dir.join(name);
    fs::write(&file_path, content).expect("failed to write script");
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(&file_path, fs::Permissions::from_mode(mode))
        .expect("failed to set permissions");
    file_path
}

fn create_config(name: &str, content: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(&format!("{name}_cfg"));
    let cfg_path = temp.path.join("failsafe.toml");
    fs::write(&cfg_path, content).expect("failed to write config");
    (temp, cfg_path)
}

#[test]
fn test_once_empty_directory() {
    let _lock = lock_test();
    let temp = TempDir::new("empty");
    let (_cfg_dir, cfg) = create_config(
        "empty",
        &format!("detectors_dir = \"{}\"\n", temp.path.display()),
    );
    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "");
}

#[test]
fn test_once_healthy() {
    let _lock = lock_test();
    let temp = TempDir::new("healthy");
    create_script(&temp.path, "10-healthy", "#!/bin/sh\nexit 0\n", true);
    let (_cfg_dir, cfg) = create_config(
        "healthy",
        &format!("detectors_dir = \"{}\"\n", temp.path.display()),
    );

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "10-healthy healthy");
}

#[test]
fn test_once_unhealthy() {
    let _lock = lock_test();
    let temp = TempDir::new("unhealthy");
    create_script(&temp.path, "20-unhealthy", "#!/bin/sh\nexit 1\n", true);
    let (_cfg_dir, cfg) = create_config(
        "unhealthy",
        &format!("detectors_dir = \"{}\"\n", temp.path.display()),
    );

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "20-unhealthy unhealthy (exit 1)");
}

#[test]
fn test_once_unknown_exit_77() {
    let _lock = lock_test();
    let temp = TempDir::new("exit77");
    create_script(&temp.path, "30-unknown", "#!/bin/sh\nexit 77\n", true);
    let (_cfg_dir, cfg) = create_config(
        "exit77",
        &format!("detectors_dir = \"{}\"\n", temp.path.display()),
    );

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "30-unknown unknown");
}

#[test]
fn test_once_non_executable() {
    let _lock = lock_test();
    let temp = TempDir::new("non_executable");
    create_script(&temp.path, "40-nonexec", "#!/bin/sh\nexit 0\n", false);
    let (_cfg_dir, cfg) = create_config(
        "non_executable",
        &format!("detectors_dir = \"{}\"\n", temp.path.display()),
    );

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("40-nonexec broken:"));
}

#[test]
fn test_once_symlink_valid_and_dangling() {
    let _lock = lock_test();
    let temp = TempDir::new("symlinks");
    let target_script = create_script(&temp.path, "target-det", "#!/bin/sh\nexit 0\n", true);
    symlink(&target_script, temp.path.join("sym-valid")).expect("failed to create symlink");
    symlink(temp.path.join("non-existent-file"), temp.path.join("sym-dangling"))
        .expect("failed to create dangling symlink");
    let (_cfg_dir, cfg) = create_config(
        "symlinks",
        &format!("detectors_dir = \"{}\"\n", temp.path.display()),
    );

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("sym-valid healthy"));
    assert!(stdout.contains("sym-dangling broken:"));
}

#[test]
fn test_once_timeout_and_process_group_cleanup() {
    let _lock = lock_test();
    let temp = TempDir::new("timeout");
    let pid_file = temp.path.join("child.pid");

    let script = format!(
        "#!/bin/sh\nsleep 100 &\necho $! > {}\nwait\n",
        pid_file.display()
    );
    create_script(&temp.path, "50-timeout", &script, true);

    let cfg_content = format!(
        "detectors_dir = \"{}\"\n\n[defaults]\ntimeout_secs = 1\n",
        temp.path.display()
    );
    let (_cfg_dir, cfg) = create_config("timeout", &cfg_content);

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "50-timeout unhealthy (timeout)");

    assert!(pid_file.exists());
    let pid_str = fs::read_to_string(&pid_file).expect("failed to read child pid file");
    let child_pid: libc::pid_t = pid_str.trim().parse().expect("failed to parse child pid");

    std::thread::sleep(Duration::from_millis(50));

    let ret = unsafe { libc::kill(child_pid, 0) };
    let errno = std::io::Error::last_os_error().raw_os_error();
    assert_eq!(ret, -1);
    assert_eq!(errno, Some(libc::ESRCH));
}

#[test]
fn test_non_root_exits_at_startup_without_dry_run() {
    let _lock = lock_test();
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return;
    }
    let temp = TempDir::new("non_root");
    let (_cfg_dir, cfg) = create_config(
        "non_root",
        &format!("detectors_dir = \"{}\"\n", temp.path.display()),
    );

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("<3>failsafe must run as root in real mode"));
}

#[test]
fn test_removed_cli_flags_exit_non_zero() {
    let _lock = lock_test();
    let cases = vec![
        vec!["--detectors-dir", "/tmp"],
        vec!["--state-dir", "/tmp"],
        vec!["--test-interval-ms", "50"],
        vec!["--test-timeout-ms", "50"],
        vec!["--test-threshold", "2"],
        vec!["--test-uptime-secs", "10"],
    ];

    for args in cases {
        let output = Command::new(binary_path())
            .args(&args)
            .output()
            .expect("failed to run failsafe");

        assert_eq!(
            output.status.code(),
            Some(1),
            "expected non-zero for args: {:?}",
            args
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("unknown argument"));
    }
}

#[test]
fn test_daemon_startup_logging_empty_dir() {
    let _lock = lock_test();
    let temp_det = TempDir::new("daemon_empty_det");
    let temp_state = TempDir::new("daemon_empty_state");

    let cfg_content = format!(
        "dry_run = true\ndetectors_dir = \"{}\"\nstate_dir = \"{}\"\n",
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("daemon_empty", &cfg_content);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in reader.lines() {
            if let Ok(l) = line
                && tx.send(l).is_err() {
                    break;
                }
        }
    });

    let mut found_watching = false;
    let mut found_no_detectors = false;
    let deadline = Instant::now() + Duration::from_secs(3);

    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if line.contains("<6>watching detectors in") {
                found_watching = true;
            }
            if line.contains("<4>no detectors found in") {
                found_no_detectors = true;
            }
            if found_watching && found_no_detectors {
                break;
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_watching, "expected '<6>watching detectors in' in stderr");
    assert!(found_no_detectors, "expected '<4>no detectors found in' in stderr");
}

#[test]
fn test_daemon_dry_run_trip_without_state_file_logs_would_reboot() {
    let _lock = lock_test();
    let temp_det = TempDir::new("dry_trip_det");
    let temp_state = TempDir::new("dry_trip_state");
    create_script(&temp_det.path, "trip-det", "#!/bin/sh\nexit 1\n", true);

    let cfg_content = format!(
        r#"
        dry_run = true
        detectors_dir = "{}"
        state_dir = "{}"

        [reboot]
        boot_grace_secs = 0

        [defaults]
        interval_secs = 1
        threshold = 1
        "#,
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("dry_trip", &cfg_content);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in reader.lines() {
            if let Ok(l) = line
                && tx.send(l).is_err() {
                    break;
                }
        }
    });

    let mut found_detector_logged = false;
    let mut found_unhealthy_run = false;
    let mut found_trip = false;
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if line.contains("<6>found detector: trip-det") {
                found_detector_logged = true;
            }
            if line.contains("detector trip-det unhealthy (exit 1), 1/1") {
                found_unhealthy_run = true;
            }
            if line.contains("would reboot: detector trip-det, exit 1, 1 consecutive failures") {
                found_trip = true;
                break;
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_detector_logged, "expected startup detector found log");
    assert!(found_unhealthy_run, "expected unhealthy run count log");
    assert!(found_trip, "expected 'would reboot' message in stderr");
}

#[test]
fn test_daemon_dry_run_high_streak_logs_would_be_refused() {
    let _lock = lock_test();
    let temp_det = TempDir::new("high_streak_det");
    let temp_state = TempDir::new("high_streak_state");
    create_script(&temp_det.path, "trip-det", "#!/bin/sh\nexit 1\n", true);

    fs::write(temp_state.path.join("state.toml"), "streak = 10\n").unwrap();

    let cfg_content = format!(
        r#"
        dry_run = true
        detectors_dir = "{}"
        state_dir = "{}"

        [reboot]
        boot_grace_secs = 20000000
        stable_after_secs = 30000000

        [defaults]
        interval_secs = 1
        threshold = 1
        "#,
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("high_streak", &cfg_content);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in reader.lines() {
            if let Ok(l) = line
                && tx.send(l).is_err() {
                    break;
                }
        }
    });

    let mut found_refused = false;
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200))
            && line.contains("would be refused: detector trip-det, exit 1, 1 consecutive failures") {
                found_refused = true;
                break;
            }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_refused, "expected 'would be refused' log for high streak in dry-run");
}

#[test]
fn test_daemon_dry_run_corrupt_state_file_logs_error_and_refuses() {
    let _lock = lock_test();
    let temp_det = TempDir::new("corrupt_det");
    let temp_state = TempDir::new("corrupt_state");
    create_script(&temp_det.path, "trip-det", "#!/bin/sh\nexit 1\n", true);

    fs::write(temp_state.path.join("state.toml"), "corrupt = {{{{").unwrap();

    let cfg_content = format!(
        r#"
        dry_run = true
        detectors_dir = "{}"
        state_dir = "{}"

        [reboot]
        boot_grace_secs = 20000000
        stable_after_secs = 30000000

        [defaults]
        interval_secs = 1
        threshold = 1
        "#,
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("corrupt", &cfg_content);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in reader.lines() {
            if let Ok(l) = line
                && tx.send(l).is_err() {
                    break;
                }
        }
    });

    let mut found_corrupt_err = false;
    let mut found_streak = false;
    let mut found_refused = false;
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if line.contains("<3>corrupt state file") {
                found_corrupt_err = true;
            }
            if line.contains("streak set to 9") {
                found_streak = true;
            }
            if line.contains("would be refused: detector trip-det, exit 1, 1 consecutive failures") {
                found_refused = true;
                break;
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_corrupt_err, "expected corrupt state file error at startup");
    assert!(found_streak, "expected streak set to 9 at startup");
    assert!(found_refused, "expected trip to be refused due to worst-case streak");
}

#[test]
fn test_daemon_dry_run_state_file_byte_identical() {
    let _lock = lock_test();
    let temp_det = TempDir::new("byte_identical_det");
    let temp_state = TempDir::new("byte_identical_state");
    create_script(&temp_det.path, "trip-det", "#!/bin/sh\nexit 1\n", true);

    let initial_content = "streak = 2\n";
    let state_file = temp_state.path.join("state.toml");
    fs::write(&state_file, initial_content).unwrap();

    let cfg_content = format!(
        r#"
        dry_run = true
        detectors_dir = "{}"
        state_dir = "{}"

        [defaults]
        interval_secs = 1
        threshold = 1
        "#,
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("byte_identical", &cfg_content);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    std::thread::sleep(Duration::from_millis(1200));

    let _ = child.kill();
    let _ = child.wait();

    let after_content = fs::read_to_string(&state_file).unwrap();
    assert_eq!(initial_content, after_content);
}

#[test]
fn test_daemon_startup_logs_previous_reboot_reason() {
    let _lock = lock_test();
    let temp_det = TempDir::new("prev_reason_det");
    let temp_state = TempDir::new("prev_reason_state");

    let state_toml = r#"
streak = 1

[last_reason]
detector = "prev-det"
consecutive_failures = 3

[last_reason.cause]
Exit = 1
"#;
    fs::write(temp_state.path.join("state.toml"), state_toml).unwrap();

    let cfg_content = format!(
        "dry_run = true\ndetectors_dir = \"{}\"\nstate_dir = \"{}\"\n",
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("prev_reason", &cfg_content);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in reader.lines() {
            if let Ok(l) = line
                && tx.send(l).is_err() {
                    break;
                }
        }
    });

    let mut found_prev_reason = false;
    let deadline = Instant::now() + Duration::from_secs(3);

    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200))
            && line.contains("<4>previous reboot was initiated by failsafe: detector prev-det, exit 1, streak 1") {
                found_prev_reason = true;
                break;
            }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_prev_reason, "expected previous reboot reason warning at startup");
}

#[test]
fn test_dry_run_does_not_create_state_dir() {
    let _lock = lock_test();
    let temp_det = TempDir::new("dry_no_state_det");
    let state_dir = std::env::temp_dir().join(format!("failsafe_nonexistent_state_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state_dir);

    let cfg_content = format!(
        "dry_run = true\ndetectors_dir = \"{}\"\nstate_dir = \"{}\"\n",
        temp_det.path.display(),
        state_dir.display()
    );
    let (_cfg_dir, cfg) = create_config("dry_no_state", &cfg_content);

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .arg("--once")
        .output()
        .expect("failed to run failsafe");

    assert!(output.status.success());
    assert!(!state_dir.exists());
}

#[test]
fn test_startup_log_config_path_and_detector_override() {
    let _lock = lock_test();
    let temp_det = TempDir::new("log_override_det");
    let temp_state = TempDir::new("log_override_state");
    create_script(&temp_det.path, "alpha", "#!/bin/sh\nexit 0\n", true);
    create_script(&temp_det.path, "beta", "#!/bin/sh\nexit 0\n", true);

    let cfg_content = format!(
        r#"
        dry_run = true
        detectors_dir = "{}"
        state_dir = "{}"

        [detector.alpha]
        threshold = 5
        "#,
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("log_override", &cfg_content);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in reader.lines() {
            if let Ok(l) = line
                && tx.send(l).is_err() {
                    break;
                }
        }
    });

    let mut found_config_path = false;
    let mut found_alpha_override = false;
    let deadline = Instant::now() + Duration::from_secs(3);

    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if line.contains(&format!("using configuration from {}", cfg.display())) {
                found_config_path = true;
            }
            if line.contains("detector alpha overrides defaults") {
                found_alpha_override = true;
            }
            if found_config_path && found_alpha_override {
                break;
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_config_path, "expected log of config path in use");
    assert!(found_alpha_override, "expected log of detector alpha override");
}

#[test]
fn test_unmatched_detector_table_halts_startup() {
    let _lock = lock_test();
    let temp_det = TempDir::new("unmatched_det");
    let temp_state = TempDir::new("unmatched_state");
    create_script(&temp_det.path, "real-det", "#!/bin/sh\nexit 0\n", true);

    let cfg_content = format!(
        r#"
        dry_run = true
        detectors_dir = "{}"
        state_dir = "{}"

        [detector.nonexistent_det]
        threshold = 5
        "#,
        temp_det.path.display(),
        temp_state.path.display()
    );
    let (_cfg_dir, cfg) = create_config("unmatched", &cfg_content);

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&cfg)
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nonexistent_det"));
    assert!(stderr.contains("no matching file"));
}
