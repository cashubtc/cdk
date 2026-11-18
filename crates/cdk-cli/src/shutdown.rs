use std::io;

/// Wait for Ctrl+C or, on Unix, SIGTERM.
pub(crate) async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
            .recv()
            .await;
        Ok(())
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<io::Result<()>>();

    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        result = terminate => result,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn shutdown_signal_handles_sigint_and_sigterm() {
        const SIGNAL_ENV: &str = "CDK_CLI_TEST_SHUTDOWN_SIGNAL";

        // Signal handlers are process-wide, so exercise each signal in its own child.
        if let Ok(signal) = std::env::var(SIGNAL_ENV) {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime should start")
                .block_on(async {
                    let mut shutdown = std::pin::pin!(shutdown_signal());
                    // Poll once to install both handlers before sending the signal.
                    std::future::poll_fn(|cx| {
                        assert!(std::future::Future::poll(shutdown.as_mut(), cx).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;

                    let status = std::process::Command::new("kill")
                        .arg(format!("-{signal}"))
                        .arg(std::process::id().to_string())
                        .status()
                        .expect("signal should be sent");
                    assert!(status.success());

                    tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
                        .await
                        .expect("signal should trigger graceful shutdown")
                        .expect("signal handler should succeed");
                });
            return;
        }

        for signal in ["INT", "TERM"] {
            let output = std::process::Command::new(
                std::env::current_exe().expect("test executable should exist"),
            )
            .args([
                "--exact",
                "shutdown::tests::shutdown_signal_handles_sigint_and_sigterm",
            ])
            .env(SIGNAL_ENV, signal)
            .output()
            .expect("child should start");
            assert!(
                output.status.success(),
                "{signal} shutdown failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
}
