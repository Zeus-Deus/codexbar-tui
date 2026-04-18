//! Run `codexbar` as a child process with a hard timeout.
//!
//! Every call spawns a fresh process, captures stdout + stderr + exit code,
//! and kills the child if it exceeds the timeout. We do not parse anything
//! here — that is parse.rs's job.
//!
//! `timings.md` in docs establishes ~15 s per call as typical and 30 s as
//! the worst-case ceiling, so the default timeout is 30 s.

use std::io::{ErrorKind, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("codexbar not on PATH or failed to start: {0}")]
    NotFound(#[source] std::io::Error),
    #[error("child i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("child timed out after {0:?}")]
    Timeout(Duration),
}

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

/// Run `codexbar <args>` and return its combined output. Kills the child
/// after `timeout` (defaults to 30 s).
///
/// Exit code 1 is **not** treated as a spawn error — codexbar returns 1 on
/// per-provider failure but still emits a valid JSON error record on stdout.
/// Callers should parse stdout regardless and inspect `Output.status` only
/// for diagnostics.
pub fn run_codexbar(args: &[&str], timeout: Option<Duration>) -> Result<Output, SpawnError> {
    let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
    let start = Instant::now();

    let mut child: Child = Command::new("codexbar")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => SpawnError::NotFound(e),
            _ => SpawnError::Io(e),
        })?;

    // Readers on dedicated threads so neither pipe can deadlock us.
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::with_capacity(32 * 1024);
        stdout_pipe.read_to_end(&mut buf)?;
        Ok::<_, std::io::Error>(buf)
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        stderr_pipe.read_to_end(&mut buf)?;
        Ok::<_, std::io::Error>(buf)
    });

    // Poll for exit with a coarse interval; timeout if deadline passes.
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SpawnError::Timeout(timeout));
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    };

    let stdout = stdout_handle.join().expect("stdout thread panicked")?;
    let stderr = stderr_handle.join().expect("stderr thread panicked")?;

    Ok(Output {
        status,
        stdout,
        stderr,
        elapsed: start.elapsed(),
    })
}

// Small wrapper for the two commands the TUI actually uses. Keeps the flag
// vocabulary in one place. See docs/cli-reference/tui-needs.md.

/// `codexbar usage --provider <id> --source cli --format json --no-color`
pub fn usage_cli(provider: &str, timeout: Option<Duration>) -> Result<Output, SpawnError> {
    run_codexbar(
        &[
            "usage",
            "--provider",
            provider,
            "--source",
            "cli",
            "--format",
            "json",
            "--no-color",
        ],
        timeout,
    )
}

/// `codexbar cost --provider <id> --format json --no-color`
pub fn cost(provider: &str, timeout: Option<Duration>) -> Result<Output, SpawnError> {
    run_codexbar(
        &[
            "cost",
            "--provider",
            provider,
            "--format",
            "json",
            "--no-color",
        ],
        timeout,
    )
}

// ---------------------------------------------------------------------------
// Tests that don't need codexbar on PATH — we wrap `true`/`sleep`/`false`
// directly via `run_generic` only inside the test module.
// ---------------------------------------------------------------------------

#[cfg(test)]
fn run_generic(
    program: &str,
    args: &[&str],
    timeout: Option<Duration>,
) -> Result<Output, SpawnError> {
    let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
    let start = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => SpawnError::NotFound(e),
            _ => SpawnError::Io(e),
        })?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let so_h = thread::spawn(move || {
        let mut b = Vec::new();
        so.read_to_end(&mut b)?;
        Ok::<_, std::io::Error>(b)
    });
    let se_h = thread::spawn(move || {
        let mut b = Vec::new();
        se.read_to_end(&mut b)?;
        Ok::<_, std::io::Error>(b)
    });
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SpawnError::Timeout(timeout));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let stdout = so_h.join().unwrap()?;
    let stderr = se_h.join().unwrap()?;
    Ok(Output {
        status,
        stdout,
        stderr,
        elapsed: start.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_zero_captures_stdout() {
        // printf is in POSIX path; avoid `echo` builtin quirks.
        let out = run_generic("/usr/bin/printf", &["hello"], None).unwrap();
        assert!(out.status.success());
        assert_eq!(&out.stdout, b"hello");
        assert!(out.stderr.is_empty());
    }

    #[test]
    fn exit_nonzero_is_not_a_spawn_error() {
        let out = run_generic("/usr/bin/false", &[], None).unwrap();
        assert!(!out.status.success());
    }

    #[test]
    fn timeout_kills_child() {
        let err = run_generic(
            "/usr/bin/sleep",
            &["5"],
            Some(Duration::from_millis(200)),
        )
        .unwrap_err();
        match err {
            SpawnError::Timeout(_) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn missing_binary_returns_not_found() {
        let err = run_generic("/nope/definitely-not-a-program", &[], None).unwrap_err();
        match err {
            SpawnError::NotFound(_) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
