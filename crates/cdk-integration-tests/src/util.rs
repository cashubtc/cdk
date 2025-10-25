//! Utility functions and process management for integration tests
//!
//! This module provides infrastructure for:
//! - Generic polling with exponential backoff
//! - Process lifecycle management with proper cleanup
//! - Command execution with logging
//!
//! Based on patterns from devimint project.

use std::ffi::OsStr;
use std::future::Future;
use std::ops::ControlFlow;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::unreachable;

use anyhow::{bail, Context, Result};
use tokio::fs::OpenOptions;
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{debug, warn};

const LOG_ITESTS: &str = "cdk_integration_tests";
const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_secs(60);

/// Simple exponential backoff iterator
pub struct Backoff {
    current: Duration,
    max: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self { current: min, max }
    }
}

impl Iterator for Backoff {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current;
        // Double the duration for next iteration, capped at max
        self.current = std::cmp::min(self.current * 2, self.max);
        Some(current)
    }
}

/// Retry until `f` succeeds or timeout is reached
///
/// - if `f` return Ok(val), this returns with Ok(val).
/// - if `f` return Err(Control::Break(err)), this returns Err(err)
/// - if `f` return Err(ControlFlow::Continue(err)), retries until timeout reached
pub async fn poll_with_timeout<Fut, R>(
    name: &str,
    timeout: Duration,
    f: impl Fn() -> Fut,
) -> Result<R>
where
    Fut: Future<Output = Result<R, ControlFlow<anyhow::Error, anyhow::Error>>>,
{
    const MIN_BACKOFF: Duration = Duration::from_millis(50);
    const MAX_BACKOFF: Duration = Duration::from_secs(1);

    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    let start = std::time::Instant::now();

    for attempt in 0u64.. {
        let attempt_start = std::time::Instant::now();
        match f().await {
            Ok(value) => return Ok(value),
            Err(ControlFlow::Break(err)) => {
                return Err(err).with_context(|| format!("polling {name}"));
            }
            Err(ControlFlow::Continue(err)) if attempt_start.duration_since(start) < timeout => {
                debug!(target: LOG_ITESTS, %attempt, error = %err, "Polling {name} failed, will retry...");
                tokio::time::sleep(backoff.next().unwrap_or(MAX_BACKOFF)).await;
            }
            Err(ControlFlow::Continue(err)) => {
                return Err(err).with_context(|| {
                    format!(
                        "Polling {name} failed after {attempt} retries (timeout: {}s)",
                        timeout.as_secs()
                    )
                });
            }
        }
    }

    unreachable!();
}

/// Retry until `f` succeeds or default timeout is reached
///
/// - if `f` return Ok(val), this returns with Ok(val).
/// - if `f` return Err(Control::Break(err)), this returns Err(err)
/// - if `f` return Err(ControlFlow::Continue(err)), retries until timeout reached
pub async fn poll<Fut, R>(name: &str, f: impl Fn() -> Fut) -> Result<R>
where
    Fut: Future<Output = Result<R, ControlFlow<anyhow::Error, anyhow::Error>>>,
{
    poll_with_timeout(name, DEFAULT_POLL_TIMEOUT, f).await
}

/// Simple polling wrapper that converts errors to ControlFlow::Continue
pub async fn poll_simple<Fut, R>(name: &str, f: impl Fn() -> Fut) -> Result<R>
where
    Fut: Future<Output = Result<R, anyhow::Error>>,
{
    poll(name, || async { f().await.map_err(ControlFlow::Continue) }).await
}

/// Kills process when all references to ProcessHandle are dropped.
///
/// NOTE: drop order is significant make sure fields in struct are declared in
/// correct order it is generally clients, process handle, deps
#[derive(Debug, Clone)]
pub struct ProcessHandle(Arc<Mutex<ProcessHandleInner>>);

impl ProcessHandle {
    pub async fn terminate(&self) -> Result<()> {
        let mut inner = self.0.lock().await;
        inner.terminate().await?;
        Ok(())
    }

    pub async fn await_terminated(&self) -> Result<()> {
        let mut inner = self.0.lock().await;
        inner.await_terminated().await?;
        Ok(())
    }

    pub async fn is_running(&self) -> bool {
        let mut inner = self.0.lock().await;
        if let Some(child) = inner.child.as_mut() {
            // Check if process has actually exited (handles zombie processes)
            match child.try_wait() {
                Ok(None) => true, // Still running
                Ok(Some(_)) => {
                    // Process has exited, clean up the child handle
                    inner.child.take();
                    false
                }
                Err(_) => false, // Error checking status, assume dead
            }
        } else {
            false
        }
    }

    pub fn pid(&self) -> Option<u32> {
        // Use try_lock since this might be called from async context
        // If lock is held, just return None (pid is only for logging)
        let inner = self.0.try_lock().ok()?;
        inner.child.as_ref().and_then(|c| c.id())
    }
}

#[derive(Debug)]
pub struct ProcessHandleInner {
    name: String,
    child: Option<Child>,
}

impl ProcessHandleInner {
    async fn terminate(&mut self) -> anyhow::Result<()> {
        if let Some(child) = self.child.as_mut() {
            debug!(
                target: LOG_ITESTS,
                name=%self.name,
                signal="SIGTERM",
                "sending signal to terminate child process"
            );

            send_sigterm(child);

            if tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_err()
            {
                debug!(
                    target: LOG_ITESTS,
                    name=%self.name,
                    signal="SIGKILL",
                    "sending signal to terminate child process"
                );

                send_sigkill(child);

                match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(err)) => {
                        bail!("Failed to terminate child process {}: {}", self.name, err);
                    }
                    Err(_) => {
                        bail!("Failed to terminate child process {}: timeout", self.name);
                    }
                }
            }
        }
        // only drop the child handle if succeeded to terminate
        self.child.take();
        Ok(())
    }

    async fn await_terminated(&mut self) -> anyhow::Result<()> {
        match self
            .child
            .as_mut()
            .expect("Process not running")
            .wait()
            .await
        {
            Ok(_status) => {
                debug!(
                    target: LOG_ITESTS,
                    name=%self.name,
                    "child process terminated"
                );
            }
            Err(err) => {
                bail!("Failed to wait for child process {}: {}", self.name, err);
            }
        }

        // only drop the child handle if succeeded to terminate
        self.child.take();
        Ok(())
    }
}

impl Drop for ProcessHandleInner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            debug!(target: LOG_ITESTS,
                name=%self.name,
                "ProcessHandleInner drop called - terminating process");

            // Send SIGTERM first
            send_sigterm(&child);

            // Try to wait briefly using try_wait (non-blocking)
            // This will succeed immediately if process dies quickly
            match child.try_wait() {
                Ok(Some(_)) => {
                    debug!(target: LOG_ITESTS,
                        name=%self.name,
                        "Process terminated successfully on drop after SIGTERM");
                }
                Ok(None) => {
                    // Process still running, escalate to SIGKILL
                    debug!(target: LOG_ITESTS,
                        name=%self.name,
                        "Process still running, sending SIGKILL");
                    send_sigkill(&child);

                    // Try once more with try_wait
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            debug!(target: LOG_ITESTS,
                                name=%self.name,
                                "Process terminated after SIGKILL");
                        }
                        Ok(None) => {
                            // Still running - let OS clean up the zombie
                            // This is safe, the process will be killed and reaped by init
                            debug!(target: LOG_ITESTS,
                                name=%self.name,
                                "Process still running, will be cleaned up by OS");
                        }
                        Err(e) => {
                            warn!(target: LOG_ITESTS,
                                name=%self.name,
                                error=%e,
                                "Error checking process status after SIGKILL");
                        }
                    }
                }
                Err(e) => {
                    warn!(target: LOG_ITESTS,
                        name=%self.name,
                        error=%e,
                        "Error checking process status on drop");
                }
            }
        } else {
            debug!(target: LOG_ITESTS,
                name=%self.name,
                "ProcessHandleInner drop called but child already terminated");
        }
    }
}

fn send_sigterm(child: &Child) {
    send_signal(child, nix::sys::signal::Signal::SIGTERM);
}

fn send_sigkill(child: &Child) {
    send_signal(child, nix::sys::signal::Signal::SIGKILL);
}

fn send_signal(child: &Child, signal: nix::sys::signal::Signal) {
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id().expect("pid should be present") as _),
        signal,
    );
}

/// Process manager for spawning and managing daemon processes
#[derive(Clone)]
pub struct ProcessManager {
    logs_dir: std::path::PathBuf,
}

impl ProcessManager {
    pub fn new(logs_dir: std::path::PathBuf) -> Self {
        std::fs::create_dir_all(&logs_dir).expect("Failed to create logs directory");
        Self { logs_dir }
    }

    /// Logs to {logs_dir}/{name}.log
    pub async fn spawn_daemon(&self, name: &str, mut cmd: Command) -> Result<ProcessHandle> {
        debug!(target: LOG_ITESTS, %name, "Spawning daemon");
        let path = self.logs_dir.join(format!("{name}.log"));
        let log = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .await?
            .into_std()
            .await;
        cmd.cmd.kill_on_drop(false); // we handle killing ourself
        cmd.cmd.stdout(log.try_clone()?);
        cmd.cmd.stderr(log);
        let child = cmd
            .cmd
            .spawn()
            .with_context(|| format!("Could not spawn: {name}"))?;
        let pid = child.id();
        let handle = ProcessHandle(Arc::new(Mutex::new(ProcessHandleInner {
            name: name.to_owned(),
            child: Some(child),
        })));
        debug!(target: LOG_ITESTS, %name, ?pid, "Daemon spawned successfully with ProcessHandle");
        Ok(handle)
    }
}

/// Command wrapper with debug tracking
pub struct Command {
    pub cmd: tokio::process::Command,
    pub args_debug: Vec<String>,
}

impl Command {
    pub fn arg<T: ToString>(mut self, arg: &T) -> Self {
        let string = arg.to_string();
        self.cmd.arg(string.clone());
        self.args_debug.push(string);
        self
    }

    pub fn args<T: ToString>(mut self, args: impl IntoIterator<Item = T>) -> Self {
        for arg in args {
            self = self.arg(&arg);
        }
        self
    }

    pub fn env<K, V>(mut self, key: K, val: V) -> Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.cmd.env(key, val);
        self
    }

    pub fn envs<I, K, V>(mut self, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.cmd.envs(env);
        self
    }

    pub fn kill_on_drop(mut self, kill: bool) -> Self {
        self.cmd.kill_on_drop(kill);
        self
    }

    fn command_debug(&self) -> String {
        self.args_debug
            .iter()
            .map(|x| x.replace(' ', "␣"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Run the command and get its output as string.
    pub async fn out_string(&mut self) -> Result<String> {
        let output = self
            .run_inner(true)
            .await
            .with_context(|| format!("command: {}", self.command_debug()))?;
        let output = String::from_utf8(output.stdout)?;
        Ok(output.trim().to_owned())
    }

    pub async fn run_inner(&mut self, expect_success: bool) -> Result<std::process::Output> {
        debug!(target: LOG_ITESTS, "> {}", self.command_debug());
        let output = self
            .cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;

        if output.status.success() != expect_success {
            bail!(
                "{}\nstdout:\n{}\nstderr:\n{}\n",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        Ok(output)
    }

    /// Run the command ignoring its output.
    pub async fn run(&mut self) -> Result<()> {
        let _ = self
            .run_inner(true)
            .await
            .with_context(|| format!("command: {}", self.command_debug()))?;
        Ok(())
    }
}

/// Trait to add `.cmd()` method to types
pub trait ToCmdExt {
    fn cmd(self) -> Command;
}

impl ToCmdExt for &str {
    fn cmd(self) -> Command {
        Command {
            cmd: tokio::process::Command::new(self),
            args_debug: vec![self.to_owned()],
        }
    }
}

impl ToCmdExt for String {
    fn cmd(self) -> Command {
        Command {
            cmd: tokio::process::Command::new(&self),
            args_debug: vec![self],
        }
    }
}

/// Easy syntax to create a Command
///
/// `cmd!(program, arg1, arg2)` expands to create a command with the given arguments
#[macro_export]
macro_rules! cmd {
    ($program:expr $(, $arg:expr)* $(,)?) => {{
        #[allow(unused)]
        use $crate::util::ToCmdExt;
        $program.cmd()
            $(.arg(&$arg))*
            .kill_on_drop(true)
            .env("RUST_BACKTRACE", "1")
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff() {
        let mut backoff = Backoff::new(Duration::from_millis(50), Duration::from_secs(1));

        assert_eq!(backoff.next(), Some(Duration::from_millis(50)));
        assert_eq!(backoff.next(), Some(Duration::from_millis(100)));
        assert_eq!(backoff.next(), Some(Duration::from_millis(200)));
        assert_eq!(backoff.next(), Some(Duration::from_millis(400)));
        assert_eq!(backoff.next(), Some(Duration::from_millis(800)));
        // Should cap at max
        assert_eq!(backoff.next(), Some(Duration::from_secs(1)));
        assert_eq!(backoff.next(), Some(Duration::from_secs(1)));
    }
}
