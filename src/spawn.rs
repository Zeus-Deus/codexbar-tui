//! Run `codexbar` as a child process with a hard timeout.
//!
//! Every call spawns a fresh process, captures stdout + stderr + exit code,
//! and kills the child if it exceeds the timeout. We do not parse anything
//! here — that is parse.rs's job.
//!
//! `timings.md` in docs establishes ~15 s per call as typical and 30 s as
//! the worst-case ceiling, so the default timeout is 30 s.

use std::io::{ErrorKind, Read};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::providers;

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

/// `codexbar usage --provider <id> --source <source> --format json --no-color`,
/// with the right `<source>` picked via `providers::preferred_source`
/// (Claude/Codex/Kiro get `cli`, most others get `api`, Vertex AI gets
/// `oauth`, Antigravity/JetBrains get `local`). Hardcoding `cli` for
/// everything was the reason Copilot, Gemini, z.ai, etc. panels all
/// errored with `"Source 'cli' is not supported for <provider>"`.
///
/// Also carries a filesystem preflight that short-circuits the subprocess
/// when we can prove from disk that the provider has no auth. See
/// `providers::known_auth_missing` — the motivation is to stop Codex CLI
/// from opening a new browser tab every 60 s when `~/.codex/auth.json`
/// doesn't exist.
pub fn usage(provider: &str, timeout: Option<Duration>) -> Result<Output, SpawnError> {
    let source = providers::preferred_source(provider);
    if providers::known_auth_missing(provider) {
        return Ok(synthetic_auth_missing_output(provider, source));
    }
    run_codexbar(
        &[
            "usage",
            "--provider",
            provider,
            "--source",
            source,
            "--format",
            "json",
            "--no-color",
        ],
        timeout,
    )
}

/// `codexbar cost --provider <id> --format json --no-color`,
/// with the same preflight as `usage_cli`. Cost normally doesn't invoke
/// the provider CLI (it scans `~/.codex/sessions/**/*.jsonl` directly)
/// but belt-and-braces: if auth is missing we have nothing meaningful
/// to show and might as well skip the subprocess.
pub fn cost(provider: &str, timeout: Option<Duration>) -> Result<Output, SpawnError> {
    if providers::known_auth_missing(provider) {
        return Ok(synthetic_auth_missing_output(provider, "local"));
    }
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

/// Fabricate an `Output` whose `stdout` is shaped exactly like codexbar's
/// real AuthMissing response (see `docs/cli-reference/usage-codex-cli.json`).
/// The downstream parse → merge pipeline then classifies it as
/// `ProviderHealth::AuthMissing` with zero subprocess invocations, so the
/// `a: show all` panel surfaces the same "run the provider CLI login"
/// hint the user would have gotten from the real call.
fn synthetic_auth_missing_output(provider: &str, source: &str) -> Output {
    // ExitStatus::from_raw on Unix: 0x0100 == exit code 1, matching the
    // real codexbar behavior when it emits an error record on stdout.
    let status = ExitStatus::from_raw(0x0100);
    let body = format!(
        r#"[{{"error":{{"code":1,"kind":"provider","message":"{provider} authentication required; run the provider CLI login"}},"provider":"{provider}","source":"{source}"}}]"#
    );
    Output {
        status,
        stdout: body.into_bytes(),
        stderr: Vec::new(),
        elapsed: Duration::ZERO,
    }
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

    #[test]
    fn synthetic_auth_missing_output_is_classified_auth_missing_downstream() {
        // Round-trip: the fake stdout we emit on preflight-fail must flow
        // through parse::parse_usage + merge::classify_error / build_snapshot
        // and land as ProviderHealth::AuthMissing. Otherwise the rest of
        // the app wouldn't know to pause the worker.
        use crate::merge::{ProviderHealth, ProviderId, build_snapshot};
        use crate::parse::parse_usage;
        use chrono::{NaiveDate, Utc};

        let out = synthetic_auth_missing_output("codex", "cli");
        assert!(!out.status.success(), "synthetic output signals failure");
        let records = parse_usage(&out.stdout).expect("parses cleanly");
        assert_eq!(records.len(), 1);
        let today: NaiveDate = "2026-04-18".parse().unwrap();
        let snap = build_snapshot(
            ProviderId::new("codex"),
            &records,
            None,
            today,
            Utc::now(),
        );
        assert!(
            matches!(snap.health, ProviderHealth::AuthMissing),
            "got {:?}",
            snap.health
        );
    }

    #[test]
    fn usage_preflight_short_circuits_codex_with_no_auth_json() {
        // Redirect CODEX_HOME to an empty tempdir so `auth.json` is
        // missing. usage MUST skip the subprocess and return the
        // synthetic AuthMissing output.
        let tmp = tempfile::tempdir().expect("tempdir");
        // SAFETY: unit tests in one module run single-threaded against
        // these env vars.
        unsafe {
            std::env::set_var("CODEX_HOME", tmp.path());
        }
        let out = usage("codex", None).expect("preflight never errors");
        // The key signal: elapsed is effectively zero because we never
        // spawned codexbar. A real run here takes ~15 seconds.
        assert!(out.elapsed < Duration::from_millis(100));
        assert!(
            std::str::from_utf8(&out.stdout)
                .unwrap()
                .contains("authentication required")
        );
        unsafe {
            std::env::remove_var("CODEX_HOME");
        }
    }
}
