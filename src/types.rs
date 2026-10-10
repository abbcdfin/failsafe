use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    Exit(i32),
    Signal(i32),
    Timeout,
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::Exit(code) => write!(f, "exit {code}"),
            Cause::Signal(sig) => write!(f, "signal {sig}"),
            Cause::Timeout => write!(f, "timeout"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Healthy,
    Unhealthy(Cause),
    Unknown,
    Broken(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Healthy => write!(f, "healthy"),
            Outcome::Unknown => write!(f, "unknown"),
            Outcome::Unhealthy(cause) => write!(f, "unhealthy ({cause})"),
            Outcome::Broken(err) => write!(f, "broken: {err}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Reboot,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Reboot => write!(f, "reboot"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub interval: Duration,
    pub timeout: Duration,
    pub threshold: u32,
    pub actions: Vec<Action>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(30),
            threshold: 3,
            actions: vec![Action::Reboot],
        }
    }
}

impl Policy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.actions.is_empty() {
            return Err("policy actions must not be empty");
        }
        if self.interval.is_zero() {
            return Err("policy interval must not be zero");
        }
        if self.timeout.is_zero() {
            return Err("policy timeout must not be zero");
        }
        if self.threshold == 0 {
            return Err("policy threshold must not be zero");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detector {
    pub name: String,
    pub path: PathBuf,
    pub policy: Policy,
}
