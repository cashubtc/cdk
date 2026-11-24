use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::Result;

pub(super) fn stop_child(child: &mut Child) -> Result<()> {
    stop_child_with_timeout(child, Duration::from_secs(30))
}

fn stop_child_with_timeout(child: &mut Child, timeout: Duration) -> Result<()> {
    // Reap already-exited children without sending a signal to a recycled PID.
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    // All three regtest daemons flush their databases and stop their subprocesses
    // on SIGTERM. Child::kill sends SIGKILL and must only be a fallback.
    let signaled = match Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
    {
        Ok(status) => status.success(),
        Err(err) => {
            tracing::warn!(
                "Could not send SIGTERM to regtest process {}: {err}",
                child.id()
            );
            false
        }
    };
    let deadline = Instant::now() + timeout;
    if signaled {
        while Instant::now() < deadline {
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            sleep(Duration::from_millis(50));
        }
    }

    tracing::warn!("Regtest process {} did not stop gracefully", child.id());
    child.kill()?;
    child.wait()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    use super::*;

    fn shell_child(trap: &str) -> Child {
        let mut child = Command::new("sh")
            .args([
                "-c",
                &format!("trap '{trap}' TERM; echo ready; while :; do :; done"),
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("start test child");
        let mut ready = String::new();
        BufReader::new(child.stdout.take().expect("child stdout"))
            .read_line(&mut ready)
            .expect("wait for signal handler");
        assert_eq!(ready.trim(), "ready");
        child
    }

    #[test]
    fn shutdown_delivers_sigterm_and_reaps_child() {
        let mut child = shell_child("exit 0");
        stop_child_with_timeout(&mut child, Duration::from_secs(2)).expect("stop child");
        assert!(child
            .try_wait()
            .expect("child status")
            .expect("exited")
            .success());
    }

    #[test]
    fn shutdown_kills_and_reaps_unresponsive_child() {
        let mut child = shell_child("");
        stop_child_with_timeout(&mut child, Duration::from_millis(20)).expect("stop child");
        assert!(child.try_wait().expect("child status").is_some());
    }

    #[test]
    fn shutdown_reaps_already_exited_child() {
        let mut child = Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("start child");
        child.wait().expect("wait for exit");
        stop_child(&mut child).expect("stop exited child");
    }
}
