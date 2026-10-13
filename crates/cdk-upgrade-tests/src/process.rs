//! Bounded child process execution and cleanup.
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use super::{check, Error, Result};

pub(crate) fn run(command: &mut Command) -> Result<()> {
    let display = format!(
        "{} {}",
        command.get_program().to_string_lossy(),
        command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("+ {display}");
    check(
        command.status()?.success(),
        format!("command exited unsuccessfully: {display}"),
    )
}

pub(crate) fn output(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    check(
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )?;
    String::from_utf8(output.stdout).map_err(|error| Error::Check(error.to_string()))
}

/// Reaps children on every error path, including failed readiness and timeouts.
#[derive(Debug)]
pub(crate) struct Process(Child);

impl Process {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self> {
        Ok(Self(command.spawn()?))
    }

    pub(crate) fn status(&mut self) -> Result<Option<ExitStatus>> {
        Ok(self.0.try_wait()?)
    }

    pub(crate) async fn wait(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.status()? {
                return Ok(status);
            }
            check(Instant::now() < deadline, "child process timed out")?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        if self.status()?.is_none() {
            run(Command::new("kill").args(["-INT", &self.0.id().to_string()]))?;
            let status = self.wait(Duration::from_secs(20)).await?;
            check(status.success(), "mint did not shut down cleanly")?;
        }
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // `wait` reaps an already exited child too. Errors cannot leave a
        // daemon running against the next scenario's database.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
