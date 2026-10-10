use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

#[test]
fn test_once_empty_directory() {
    let temp = TempDir::new("empty");
    let output = Command::new(binary_path())
        .arg("--once")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .output()
        .expect("failed to run failsafe");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "");
}

#[test]
fn test_once_healthy() {
    let temp = TempDir::new("healthy");
    create_script(&temp.path, "10-healthy", "#!/bin/sh\nexit 0\n", true);

    let output = Command::new(binary_path())
        .arg("--once")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .output()
        .expect("failed to run failsafe");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "10-healthy healthy");
}

#[test]
fn test_once_unhealthy() {
    let temp = TempDir::new("unhealthy");
    create_script(&temp.path, "20-unhealthy", "#!/bin/sh\nexit 1\n", true);

    let output = Command::new(binary_path())
        .arg("--once")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "20-unhealthy unhealthy (exit 1)");
}

#[test]
fn test_once_unknown_exit_77() {
    let temp = TempDir::new("exit77");
    create_script(&temp.path, "30-unknown", "#!/bin/sh\nexit 77\n", true);

    let output = Command::new(binary_path())
        .arg("--once")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .output()
        .expect("failed to run failsafe");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "30-unknown unknown");
}

#[test]
fn test_once_non_executable() {
    let temp = TempDir::new("non_executable");
    create_script(&temp.path, "40-nonexec", "#!/bin/sh\nexit 0\n", false);

    let output = Command::new(binary_path())
        .arg("--once")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("40-nonexec broken:"));
}

#[test]
fn test_once_symlink_valid_and_dangling() {
    let temp = TempDir::new("symlinks");
    let target_script = create_script(&temp.path, "target-det", "#!/bin/sh\nexit 0\n", true);
    symlink(&target_script, temp.path.join("sym-valid")).expect("failed to create symlink");
    symlink(temp.path.join("non-existent-file"), temp.path.join("sym-dangling"))
        .expect("failed to create dangling symlink");

    let output = Command::new(binary_path())
        .arg("--once")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .output()
        .expect("failed to run failsafe");

    // Has dangling symlink, so exits non-zero
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("sym-valid healthy"));
    assert!(stdout.contains("sym-dangling broken:"));
}

#[test]
fn test_once_timeout_and_process_group_cleanup() {
    let temp = TempDir::new("timeout");
    let pid_file = temp.path.join("child.pid");

    let script = format!(
        "#!/bin/sh\nsleep 100 &\necho $! > {}\nwait\n",
        pid_file.display()
    );
    create_script(&temp.path, "50-timeout", &script, true);

    let output = Command::new(binary_path())
        .arg("--once")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .arg("--test-timeout-ms")
        .arg("200")
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
fn test_refuse_real_reboot_without_dry_run() {
    let temp = TempDir::new("refuse_reboot");
    let output = Command::new(binary_path())
        .arg("--detectors-dir")
        .arg(&temp.path)
        .output()
        .expect("failed to run failsafe");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("<3>reboot is not implemented; run with --dry-run"));
}

#[test]
fn test_invalid_test_flags_exit_non_zero() {
    let temp = TempDir::new("invalid_flags");

    let cases = vec![
        vec!["--test-interval-ms", "0"],
        vec!["--test-interval-ms", "abc"],
        vec!["--test-timeout-ms", "0"],
        vec!["--test-timeout-ms", "xyz"],
        vec!["--test-threshold", "0"],
        vec!["--test-threshold", "bad"],
    ];

    for args in cases {
        let output = Command::new(binary_path())
            .arg("--dry-run")
            .arg("--detectors-dir")
            .arg(&temp.path)
            .args(&args)
            .output()
            .expect("failed to run failsafe");

        assert_eq!(
            output.status.code(),
            Some(1),
            "expected non-zero for args: {:?}",
            args
        );
    }
}

#[test]
fn test_daemon_startup_logging_empty_dir() {
    let temp = TempDir::new("daemon_empty");

    let mut child = Command::new(binary_path())
        .arg("--dry-run")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let start = Instant::now();
    let mut found_watching = false;
    let mut found_no_detectors = false;

    for line in reader.lines() {
        if let Ok(line) = line {
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
        if start.elapsed() > Duration::from_secs(3) {
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_watching, "expected '<6>watching detectors in' in stderr");
    assert!(found_no_detectors, "expected '<4>no detectors found in' in stderr");
}

#[test]
fn test_daemon_loop_logging_and_tripping() {
    let temp = TempDir::new("daemon_trip");
    create_script(&temp.path, "trip-det", "#!/bin/sh\nexit 1\n", true);

    let mut child = Command::new(binary_path())
        .arg("--dry-run")
        .arg("--detectors-dir")
        .arg(&temp.path)
        .arg("--test-interval-ms")
        .arg("50")
        .arg("--test-threshold")
        .arg("2")
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn failsafe daemon");

    let stderr = child.stderr.take().expect("failed to capture stderr");
    let reader = BufReader::new(stderr);

    let start = Instant::now();
    let mut found_detector_logged = false;
    let mut found_unhealthy_run = false;
    let mut found_trip = false;

    for line in reader.lines() {
        if let Ok(line) = line {
            if line.contains("<6>found detector: trip-det") {
                found_detector_logged = true;
            }
            if line.contains("detector trip-det unhealthy (exit 1), 1/2") {
                found_unhealthy_run = true;
            }
            if line.contains("would reboot: detector trip-det, exit 1, 2 consecutive failures") {
                found_trip = true;
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(5) {
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found_detector_logged, "expected startup detector found log");
    assert!(found_unhealthy_run, "expected unhealthy run count log");
    assert!(found_trip, "expected 'would reboot' message in stderr");
}
