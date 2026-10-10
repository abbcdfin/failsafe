use std::fs;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::types::{Cause, Detector, Outcome, Policy};

pub fn run(path: &Path, timeout: Duration) -> Outcome {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return Outcome::Broken(format!("cannot resolve detector: {e}")),
    };
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

pub fn discover(dir: &Path, default_policy: &Policy) -> Result<Vec<Detector>, std::io::Error> {
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
        .map(|(name, path)| Detector {
            name,
            path,
            policy: default_policy.clone(),
        })
        .collect();

    Ok(detectors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn test_run_healthy() {
        let outcome = run(Path::new("/bin/true"), Duration::from_secs(5));
        assert_eq!(outcome, Outcome::Healthy);
    }

    #[test]
    fn test_run_unhealthy_exit() {
        let outcome = run(Path::new("/bin/false"), Duration::from_secs(5));
        assert_eq!(outcome, Outcome::Unhealthy(Cause::Exit(1)));
    }

    #[test]
    fn test_run_non_existent() {
        let outcome = run(Path::new("/nonexistent/file"), Duration::from_secs(5));
        match outcome {
            Outcome::Broken(_) => {}
            other => panic!("expected Broken, got {:?}", other),
        }
    }

    #[test]
    fn test_discover_sorted_and_symlinks() {
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

        let policy = Policy::default();
        let detectors = discover(&tmp, &policy).unwrap();

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

    fn tempfile_dir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
