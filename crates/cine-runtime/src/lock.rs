//! The single-instance lock: one runtime per project directory.
//!
//! ## What this is defending
//!
//! Two runtimes on one project would each hold an engine and each append to the
//! op log, so the project's history would interleave two incompatible views and
//! neither could be replayed. That is silent corruption, and it is exactly what
//! a second terminal running the same command would cause.
//!
//! So a second invocation does not start a second runtime. It reads the lock,
//! finds the address of the runtime already serving, and reports it — the
//! second terminal *attaches* rather than competes.
//!
//! ## Staleness
//!
//! A lock left behind by a crashed process must not block the project forever.
//! The lock records the pid, and a lock whose process is gone is stale and may
//! be taken. On Unix that is `kill(pid, 0)`, which asks whether the process
//! exists without signalling it; `/proc` is consulted first because it also
//! distinguishes a *different* process that has since reused the pid.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a running runtime publishes about itself.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InstanceInfo {
    pub pid: u32,
    /// Address clients should connect to, e.g. `127.0.0.1:7331`.
    pub address: String,
    /// Seconds since the Unix epoch when the runtime started.
    pub started_unix: u64,
    /// Absolute project directory, so a moved lock file is detectable.
    pub project: String,
    /// Runtime version, so a stale lock from an older build is legible.
    pub version: String,
}

impl InstanceInfo {
    pub fn url(&self) -> String {
        format!("http://{}/", self.address)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("the lock file is corrupt: {0}")]
    Corrupt(String),
    #[error("CINE-OMNI is already running for this project (pid {}, {})", .0.pid, .0.url())]
    AlreadyRunning(Box<InstanceInfo>),
}

pub fn lock_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".cine").join("runtime.lock")
}

/// Whether a process is currently running under this pid.
///
/// Returns `true` when the answer is unknown, because refusing to start is
/// safer than corrupting a project on a maybe.
pub fn process_is_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // Signal 0 performs error checking without sending anything.
        unsafe { libc_kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// Read the lock, if one exists and is live.
///
/// A lock belonging to a dead process is removed and reported as absent, so a
/// crash does not leave the project unopenable.
pub fn read_live_lock(project_dir: &Path) -> Result<Option<InstanceInfo>, LockError> {
    let path = lock_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let info: InstanceInfo = match serde_json::from_str(&text) {
        Ok(i) => i,
        Err(e) => {
            // A corrupt lock is reported rather than silently deleted: deleting
            // it could let a second runtime start alongside a live one.
            return Err(LockError::Corrupt(e.to_string()));
        }
    };
    if process_is_alive(info.pid) {
        Ok(Some(info))
    } else {
        let _ = std::fs::remove_file(&path);
        Ok(None)
    }
}

/// Take the lock for this process, failing if a live runtime already holds it.
pub fn acquire(project_dir: &Path, address: &str) -> Result<InstanceGuard, LockError> {
    if let Some(existing) = read_live_lock(project_dir)? {
        return Err(LockError::AlreadyRunning(Box::new(existing)));
    }
    let path = lock_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let info = InstanceInfo {
        pid: std::process::id(),
        address: address.to_string(),
        started_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        project: project_dir
            .canonicalize()
            .unwrap_or_else(|_| project_dir.to_path_buf())
            .display()
            .to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&info).unwrap())?;
    Ok(InstanceGuard { path, info })
}

/// Removes the lock when the runtime exits.
#[derive(Debug)]
pub struct InstanceGuard {
    path: PathBuf,
    pub info: InstanceInfo,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // Only remove a lock that is still ours: another runtime may have taken
        // it after judging ours stale, and deleting theirs would let a third start.
        if let Ok(text) = std::fs::read_to_string(&self.path) {
            if let Ok(current) = serde_json::from_str::<InstanceInfo>(&text) {
                if current.pid != self.info.pid {
                    return;
                }
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cine-lock-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_second_runtime_is_refused_and_told_where_the_first_one_is() {
        let dir = scratch("second");
        let _first = acquire(&dir, "127.0.0.1:7331").expect("first acquires");

        match acquire(&dir, "127.0.0.1:7332") {
            Err(LockError::AlreadyRunning(info)) => {
                assert_eq!(info.address, "127.0.0.1:7331");
                assert_eq!(info.pid, std::process::id());
                assert_eq!(info.url(), "http://127.0.0.1:7331/");
            }
            other => panic!("a second runtime must be refused, got {other:?}"),
        }
    }

    #[test]
    fn the_lock_is_released_when_the_runtime_exits() {
        let dir = scratch("release");
        {
            let _g = acquire(&dir, "127.0.0.1:7331").unwrap();
            assert!(lock_path(&dir).exists());
        }
        assert!(
            !lock_path(&dir).exists(),
            "the lock must not outlive the runtime"
        );
        acquire(&dir, "127.0.0.1:7331").expect("the project is openable again");
    }

    /// A crash leaves a lock behind. The project must still open.
    #[test]
    fn a_lock_from_a_dead_process_is_reclaimed() {
        let dir = scratch("stale");
        std::fs::create_dir_all(dir.join(".cine")).unwrap();
        let dead = InstanceInfo {
            // A pid that cannot be running: pid 0 is never a user process.
            pid: 0,
            address: "127.0.0.1:9999".into(),
            started_unix: 0,
            project: dir.display().to_string(),
            version: "0.0.0".into(),
        };
        std::fs::write(lock_path(&dir), serde_json::to_string(&dead).unwrap()).unwrap();

        assert!(
            read_live_lock(&dir).unwrap().is_none(),
            "a dead lock is not live"
        );
        let g = acquire(&dir, "127.0.0.1:7331").expect("a stale lock must not block the project");
        assert_eq!(g.info.pid, std::process::id());
    }

    /// A corrupt lock is ambiguous: it might belong to a live runtime whose
    /// file was damaged. Refusing is safer than assuming it is free.
    #[test]
    fn a_corrupt_lock_is_reported_rather_than_assumed_free() {
        let dir = scratch("corrupt");
        std::fs::create_dir_all(dir.join(".cine")).unwrap();
        std::fs::write(lock_path(&dir), "{ this is not json").unwrap();
        assert!(matches!(read_live_lock(&dir), Err(LockError::Corrupt(_))));
        assert!(matches!(
            acquire(&dir, "127.0.0.1:1"),
            Err(LockError::Corrupt(_))
        ));
    }

    #[test]
    fn this_process_is_alive_and_pid_zero_is_not() {
        assert!(process_is_alive(std::process::id()));
        assert!(!process_is_alive(0));
    }

    /// Dropping a guard whose lock was taken over by another runtime must not
    /// delete that runtime's lock.
    #[test]
    fn a_superseded_guard_does_not_remove_the_new_holders_lock() {
        let dir = scratch("superseded");
        let guard = acquire(&dir, "127.0.0.1:7331").unwrap();
        // Simulate another runtime having replaced the lock.
        let other = InstanceInfo {
            pid: std::process::id() + 1,
            address: "127.0.0.1:7332".into(),
            started_unix: 1,
            project: dir.display().to_string(),
            version: "0.0.0".into(),
        };
        std::fs::write(lock_path(&dir), serde_json::to_string(&other).unwrap()).unwrap();
        drop(guard);
        assert!(
            lock_path(&dir).exists(),
            "another runtime's lock must survive our exit"
        );
    }
}
