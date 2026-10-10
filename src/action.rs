use std::fs;
use std::io;
use std::process::{Command, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::log_error;

pub trait RebootChild {
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, io::Error>;
    fn kill(&mut self) -> Result<(), io::Error>;
}

impl RebootChild for std::process::Child {
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, io::Error> {
        self.try_wait()
    }
    fn kill(&mut self) -> Result<(), io::Error> {
        self.kill()
    }
}

pub trait Rebooter {
    fn start_sync(&mut self) -> Receiver<()>;
    fn request_reboot(&mut self) -> Result<Box<dyn RebootChild>, io::Error>;
    fn force_reboot(&mut self) -> Result<(), io::Error>;
}

pub struct SystemRebooter;

impl Rebooter for SystemRebooter {
    fn start_sync(&mut self) -> Receiver<()> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            unsafe {
                libc::sync();
            }
            let _ = tx.send(());
        });
        rx
    }

    fn request_reboot(&mut self) -> Result<Box<dyn RebootChild>, io::Error> {
        let child = Command::new("systemctl").arg("reboot").spawn()?;
        Ok(Box::new(child))
    }

    fn force_reboot(&mut self) -> Result<(), io::Error> {
        unsafe {
            let ret = libc::reboot(libc::RB_AUTOBOOT);
            if ret != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

pub fn read_boot_id() -> Option<String> {
    match fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        Ok(s) => Some(s.trim().to_string()),
        Err(e) => {
            log_error!("failed to read /proc/sys/kernel/random/boot_id: {e}");
            None
        }
    }
}

pub fn read_uptime() -> Duration {
    match fs::read_to_string("/proc/uptime") {
        Ok(content) => {
            let first = content.split_whitespace().next();
            match first.and_then(|s| s.parse::<f64>().ok()) {
                Some(secs) => Duration::from_secs_f64(secs),
                None => {
                    log_error!("failed to parse /proc/uptime: {content:?}");
                    Duration::ZERO
                }
            }
        }
        Err(e) => {
            log_error!("failed to read /proc/uptime: {e}");
            Duration::ZERO
        }
    }
}

fn bounded_sync(rebooter: &mut dyn Rebooter, timeout: Duration) {
    let rx = rebooter.start_sync();
    if rx.recv_timeout(timeout).is_err() {
        log_error!(
            "sync timed out after {} s, proceeding anyway",
            timeout.as_secs()
        );
    }
}

pub fn execute_reboot(
    rebooter: &mut dyn Rebooter,
    deadline: Duration,
    force_only: bool,
) -> Result<(), io::Error> {
    execute_reboot_with_timeouts(
        rebooter,
        deadline,
        force_only,
        Duration::from_secs(5),
        Duration::from_millis(50),
    )
}

pub fn execute_reboot_with_timeouts(
    rebooter: &mut dyn Rebooter,
    deadline: Duration,
    force_only: bool,
    sync_timeout: Duration,
    poll_interval: Duration,
) -> Result<(), io::Error> {
    bounded_sync(rebooter, sync_timeout);

    if !force_only {
        match rebooter.request_reboot() {
            Ok(mut child) => {
                let start = std::time::Instant::now();
                let mut exited_cleanly = false;

                loop {
                    if !exited_cleanly {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                if status.success() {
                                    exited_cleanly = true;
                                } else {
                                    log_error!(
                                        "request reboot (systemctl) failed: exited with status {status}, falling back to forced reboot"
                                    );
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                log_error!(
                                    "request reboot (systemctl) failed: wait error {e}, falling back to forced reboot"
                                );
                                let _ = child.kill();
                                break;
                            }
                        }
                    }

                    if start.elapsed() >= deadline {
                        if !exited_cleanly {
                            log_error!(
                                "request reboot (systemctl) timed out after {} s, killing and falling back to forced reboot",
                                deadline.as_secs()
                            );
                            let _ = child.kill();
                        }
                        break;
                    }

                    let remaining = deadline.saturating_sub(start.elapsed());
                    std::thread::sleep(poll_interval.min(remaining));
                }
            }
            Err(e) => {
                log_error!(
                    "request reboot (systemctl) failed: {e}, falling back to forced reboot"
                );
            }
        }
    }

    bounded_sync(rebooter, sync_timeout);
    rebooter.force_reboot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct FakeChild {
        status: Option<ExitStatus>,
        killed: Arc<AtomicBool>,
    }

    impl RebootChild for FakeChild {
        fn try_wait(&mut self) -> Result<Option<ExitStatus>, io::Error> {
            Ok(self.status)
        }
        fn kill(&mut self) -> Result<(), io::Error> {
            self.killed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeRebooter {
        calls: Vec<String>,
        request_error: bool,
        child_status: Option<Option<ExitStatus>>,
        child_killed: Arc<AtomicBool>,
        force_error: bool,
        sync_hang: bool,
        _sync_tx: Option<mpsc::Sender<()>>,
    }

    impl Rebooter for FakeRebooter {
        fn start_sync(&mut self) -> Receiver<()> {
            self.calls.push("sync".to_string());
            let (tx, rx) = mpsc::channel();
            if self.sync_hang {
                self._sync_tx = Some(tx);
            } else {
                let _ = tx.send(());
            }
            rx
        }

        fn request_reboot(&mut self) -> Result<Box<dyn RebootChild>, io::Error> {
            self.calls.push("request_reboot".to_string());
            if self.request_error {
                Err(io::Error::new(io::ErrorKind::NotFound, "systemctl not found"))
            } else {
                let status = self
                    .child_status
                    .unwrap_or_else(|| Some(ExitStatus::from_raw(0)));
                Ok(Box::new(FakeChild {
                    status,
                    killed: self.child_killed.clone(),
                }))
            }
        }

        fn force_reboot(&mut self) -> Result<(), io::Error> {
            self.calls.push("force_reboot".to_string());
            if self.force_error {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "reboot failed",
                ))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn test_normal_path() {
        let mut fake = FakeRebooter::default();
        let res = execute_reboot(&mut fake, Duration::ZERO, false);
        assert!(res.is_ok());
        assert_eq!(
            fake.calls,
            vec!["sync", "request_reboot", "sync", "force_reboot"]
        );
    }

    #[test]
    fn test_request_cannot_be_spawned_goes_to_force() {
        let mut fake = FakeRebooter {
            request_error: true,
            ..Default::default()
        };
        let res = execute_reboot(&mut fake, Duration::ZERO, false);
        assert!(res.is_ok());
        assert_eq!(
            fake.calls,
            vec!["sync", "request_reboot", "sync", "force_reboot"]
        );
    }

    #[test]
    fn test_request_exits_non_zero_goes_to_force_without_waiting() {
        let mut fake = FakeRebooter {
            child_status: Some(Some(ExitStatus::from_raw(1 << 8))), // exit 1
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let res = execute_reboot_with_timeouts(
            &mut fake,
            Duration::from_secs(5),
            false,
            Duration::from_millis(50),
            Duration::from_millis(10),
        );
        assert!(res.is_ok());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(
            fake.calls,
            vec!["sync", "request_reboot", "sync", "force_reboot"]
        );
    }

    #[test]
    fn test_request_never_returns_child_killed_then_force() {
        let killed = Arc::new(AtomicBool::new(false));
        let mut fake = FakeRebooter {
            child_status: Some(None), // never returns
            child_killed: killed.clone(),
            ..Default::default()
        };
        let res = execute_reboot_with_timeouts(
            &mut fake,
            Duration::from_millis(50),
            false,
            Duration::from_millis(50),
            Duration::from_millis(10),
        );
        assert!(res.is_ok());
        assert!(killed.load(Ordering::SeqCst));
        assert_eq!(
            fake.calls,
            vec!["sync", "request_reboot", "sync", "force_reboot"]
        );
    }

    #[test]
    fn test_sync_never_returns_sequence_completes() {
        let mut fake = FakeRebooter {
            sync_hang: true,
            ..Default::default()
        };
        let res = execute_reboot_with_timeouts(
            &mut fake,
            Duration::ZERO,
            false,
            Duration::from_millis(20),
            Duration::from_millis(10),
        );
        assert!(res.is_ok());
        assert!(fake.calls.contains(&"force_reboot".to_string()));
    }

    #[test]
    fn test_force_only_skips_request() {
        let mut fake = FakeRebooter::default();
        let res = execute_reboot(&mut fake, Duration::ZERO, true);
        assert!(res.is_ok());
        assert_eq!(fake.calls, vec!["sync", "sync", "force_reboot"]);
    }

    #[test]
    fn test_force_fails_returns_error() {
        let mut fake = FakeRebooter {
            request_error: true,
            force_error: true,
            ..Default::default()
        };
        let res = execute_reboot(&mut fake, Duration::ZERO, false);
        assert!(res.is_err());
    }

    #[test]
    fn test_read_uptime_host() {
        let uptime = read_uptime();
        assert!(uptime.as_secs() > 0);
    }
}
