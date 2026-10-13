use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::types::{Cause, Detector, Outcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionError {
    NotOwnedByRoot(u32),
    GroupOrWorldWritable(u32),
}

impl fmt::Display for PermissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PermissionError::NotOwnedByRoot(uid) => {
                write!(f, "must be owned by root (uid 0), but is owned by {uid}")
            }
            PermissionError::GroupOrWorldWritable(mode) => {
                write!(f, "must not be writable by group or others (mode: {:04o})", mode & 0o7777)
            }
        }
    }
}

pub fn check_permissions(euid: u32, file_uid: u32, file_mode: u32) -> Result<(), PermissionError> {
    if euid != 0 {
        return Ok(());
    }
    if file_uid != 0 {
        return Err(PermissionError::NotOwnedByRoot(file_uid));
    }
    if (file_mode & 0o022) != 0 {
        return Err(PermissionError::GroupOrWorldWritable(file_mode & 0o7777));
    }
    Ok(())
}

pub fn run(path: &Path, timeout: Duration) -> Outcome {
    let euid = unsafe { libc::geteuid() };

    if let Some(dir) = path.parent() {
        match fs::metadata(dir) {
            Ok(dir_meta) => {
                if let Err(err) = check_permissions(euid, dir_meta.uid(), dir_meta.mode()) {
                    return Outcome::Broken(format!("detectors directory {} {err}", dir.display()));
                }
            }
            Err(e) => {
                return Outcome::Broken(format!(
                    "cannot stat detectors directory {}: {e}",
                    dir.display()
                ));
            }
        }
    }

    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return Outcome::Broken(format!("cannot resolve detector: {e}")),
    };
    if let Err(err) = check_permissions(euid, meta.uid(), meta.mode()) {
        return Outcome::Broken(format!("detector {} {err}", path.display()));
    }
    if !meta.is_file() {
        return Outcome::Broken("detector is not a regular file".to_string());
    }

    let mut cmd = Command::new(path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .current_dir("/")
        .process_group(0);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Outcome::Broken(e.to_string()),
    };

    let pid = child.id() as libc::pid_t;
    let start = Instant::now();
    let poll_interval = Duration::from_millis(10);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return map_status(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    return Outcome::Unhealthy(Cause::Timeout);
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                let _ = child.wait();
                return Outcome::Broken(e.to_string());
            }
        }
    }
}

fn map_status(status: ExitStatus) -> Outcome {
    if let Some(code) = status.code() {
        match code {
            0 => Outcome::Healthy,
            77 => Outcome::Unknown,
            c => Outcome::Unhealthy(Cause::Exit(c)),
        }
    } else if let Some(sig) = status.signal() {
        Outcome::Unhealthy(Cause::Signal(sig))
    } else {
        Outcome::Unhealthy(Cause::Exit(-1))
    }
}

pub fn discover(dir: &Path, config: &Config) -> Result<Vec<Detector>, std::io::Error> {
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(dir)?;

    for entry in read_dir {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            continue;
        }
        if file_type.is_file() || file_type.is_symlink() {
            let name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => entry.file_name().to_string_lossy().into_owned(),
            };
            entries.push((name, entry.path()));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let detectors = entries
        .into_iter()
        .map(|(name, path)| {
            let policy = config.policy_for(&name);
            Detector {
                name,
                path,
                policy,
            }
        })
        .collect();

    Ok(detectors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::{Mutex, MutexGuard};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_test() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_run_healthy() {
        let _lock = lock_test();
        let outcome = run(Path::new("/bin/true"), Duration::from_secs(5));
        assert_eq!(outcome, Outcome::Healthy);
    }

    #[test]
    fn test_run_unhealthy_exit() {
        let _lock = lock_test();
        let outcome = run(Path::new("/bin/false"), Duration::from_secs(5));
        assert_eq!(outcome, Outcome::Unhealthy(Cause::Exit(1)));
    }

    #[test]
    fn test_run_non_existent() {
        let _lock = lock_test();
        let outcome = run(Path::new("/nonexistent/file"), Duration::from_secs(5));
        match outcome {
            Outcome::Broken(_) => {}
            other => panic!("expected Broken, got {:?}", other),
        }
    }

    #[test]
    fn test_discover_sorted_and_symlinks() {
        let _lock = lock_test();
        let tmp = tempfile_dir("test_discover");
        let path_b = tmp.join("b-det");
        let path_a = tmp.join("a-det");
        let path_symlink = tmp.join("c-symlink");
        let path_dangling = tmp.join("d-dangling");
        let subdir = tmp.join("sub-dir");

        fs::write(&path_b, "#!/bin/sh\nexit 0").unwrap();
        fs::set_permissions(&path_b, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&path_a, "#!/bin/sh\nexit 0").unwrap();
        fs::set_permissions(&path_a, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&path_b, &path_symlink).unwrap();
        symlink(tmp.join("does-not-exist"), &path_dangling).unwrap();
        fs::create_dir(&subdir).unwrap();

        let config = Config::default();
        let detectors = discover(&tmp, &config).unwrap();

        assert_eq!(detectors.len(), 4);
        assert_eq!(detectors[0].name, "a-det");
        assert_eq!(detectors[1].name, "b-det");
        assert_eq!(detectors[2].name, "c-symlink");
        assert_eq!(detectors[3].name, "d-dangling");

        // Running valid symlink succeeds
        let outcome = run(&detectors[2].path, Duration::from_secs(5));
        assert_eq!(outcome, Outcome::Healthy);

        // Running dangling symlink reports Broken
        let outcome = run(&detectors[3].path, Duration::from_secs(5));
        match outcome {
            Outcome::Broken(_) => {}
            other => panic!("expected Broken for dangling symlink, got {:?}", other),
        }

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn test_check_permissions_non_root_always_allowed() {
        assert_eq!(check_permissions(1000, 1000, 0o777), Ok(()));
        assert_eq!(check_permissions(1000, 0, 0o666), Ok(()));
    }

    #[test]
    fn test_check_permissions_root_valid() {
        assert_eq!(check_permissions(0, 0, 0o755), Ok(()));
        assert_eq!(check_permissions(0, 0, 0o644), Ok(()));
        assert_eq!(check_permissions(0, 0, 0o700), Ok(()));
    }

    #[test]
    fn test_check_permissions_root_wrong_owner() {
        assert_eq!(
            check_permissions(0, 1000, 0o755),
            Err(PermissionError::NotOwnedByRoot(1000))
        );
    }

    #[test]
    fn test_check_permissions_root_writable_by_group_or_others() {
        assert_eq!(
            check_permissions(0, 0, 0o775),
            Err(PermissionError::GroupOrWorldWritable(0o775))
        );
        assert_eq!(
            check_permissions(0, 0, 0o777),
            Err(PermissionError::GroupOrWorldWritable(0o777))
        );
        assert_eq!(
            check_permissions(0, 0, 0o664),
            Err(PermissionError::GroupOrWorldWritable(0o664))
        );
        assert_eq!(
            check_permissions(0, 0, 0o646),
            Err(PermissionError::GroupOrWorldWritable(0o646))
        );
    }

    fn tempfile_dir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
