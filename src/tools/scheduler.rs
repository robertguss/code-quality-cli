//! Run shell commands in parallel, each in its own process group with a
//! timeout that kills the whole group.

use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Output kept in memory per stream; the rest is drained and dropped.
const MAX_CAPTURE: usize = 8 * 1024 * 1024;

/// One shell command to run.
#[derive(Clone, Debug)]
pub struct Job {
    pub command: String,
    pub cwd: PathBuf,
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    /// Killed by a signal it did not get from us.
    Signal(i32),
    /// Ran past its timeout; its process group was killed.
    TimedOut,
    /// `sh` could not be started or waited on.
    SpawnFailed(String),
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

/// How many jobs run at once. Tools mostly wait on I/O or run their own
/// threads, so small machines still get a floor of 4.
pub fn workers() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(4)
}

/// Apply `f` to every item on a bounded pool of threads, keeping input order.
pub fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new(items.iter().map(|_| None).collect());
    thread::scope(|s| {
        for _ in 0..workers().min(items.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    results.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|r| r.expect("every item ran"))
        .collect()
}

/// Run `sh -c <command>` with stdin closed, reading stdout and stderr
/// concurrently so a chatty tool cannot block on a full pipe.
pub fn run(job: &Job) -> Outcome {
    let start = Instant::now();
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(&job.command)
        .current_dir(&job.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            return Outcome {
                exit: Exit::SpawnFailed(e.to_string()),
                stdout: Vec::new(),
                stderr: Vec::new(),
                elapsed: start.elapsed(),
            };
        }
    };
    let pgid = child.id() as i32;
    let stdout = child.stdout.take().map(|s| thread::spawn(move || drain(s)));
    let stderr = child.stderr.take().map(|s| thread::spawn(move || drain(s)));
    let deadline = start + job.timeout;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break match (status.code(), status.signal()) {
                    (Some(code), _) => Exit::Code(code),
                    (None, Some(sig)) => Exit::Signal(sig),
                    (None, None) => Exit::SpawnFailed("unknown exit status".into()),
                };
            }
            Ok(None) if Instant::now() >= deadline => {
                kill_group(pgid);
                let _ = child.wait();
                break Exit::TimedOut;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                kill_group(pgid);
                let _ = child.wait();
                break Exit::SpawnFailed(e.to_string());
            }
        }
    };
    // The tool is done; background processes it left behind would hold the
    // pipes open forever, so they go too.
    kill_group(pgid);
    let join = |h: Option<thread::JoinHandle<Vec<u8>>>| {
        h.map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    };
    Outcome {
        exit,
        stdout: join(stdout),
        stderr: join(stderr),
        elapsed: start.elapsed(),
    }
}

fn kill_group(pgid: i32) {
    // SAFETY: kill(2) has no memory effects; a missing group returns ESRCH.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

fn drain(mut r: impl Read) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut dropped = false;
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_CAPTURE.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
                dropped |= n > room;
            }
        }
    }
    if dropped {
        kept.extend_from_slice(b"\n[qgate: output truncated]\n");
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(command: &str, secs: u64) -> Job {
        Job {
            command: command.into(),
            cwd: std::env::temp_dir(),
            timeout: Duration::from_secs(secs),
        }
    }

    #[test]
    fn captures_both_streams_and_exit_code() {
        let o = run(&job("echo out; echo err >&2; exit 3", 10));
        assert_eq!(o.exit, Exit::Code(3));
        assert_eq!(o.stdout, b"out\n");
        assert_eq!(o.stderr, b"err\n");
    }

    #[test]
    fn large_output_does_not_deadlock() {
        let o = run(&job(
            "head -c 3000000 /dev/zero; head -c 3000000 /dev/zero >&2",
            20,
        ));
        assert_eq!(o.exit, Exit::Code(0));
        assert_eq!(o.stdout.len(), 3_000_000);
        assert_eq!(o.stderr.len(), 3_000_000);
    }

    #[test]
    fn signal_death_is_reported() {
        let o = run(&job("kill -TERM $$", 10));
        assert_eq!(o.exit, Exit::Signal(libc::SIGTERM));
    }

    #[test]
    fn parallel_map_keeps_order() {
        let v: Vec<u32> = (0..20).collect();
        assert_eq!(
            parallel_map(&v, |x| x * 2),
            (0..20).map(|x| x * 2).collect::<Vec<_>>()
        );
    }
}
