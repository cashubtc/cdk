//! Federated CDK Cashu wallet and real-time member activity dashboard.

mod app;
mod telemetry;
mod ui;
mod wallet;

use std::fs;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use app::{App, AppCommand, AppEvent};
use cdk::mint_url::MintUrl;
use clap::{Parser, Subcommand};
use crossterm::event;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, event::DisableBracketedPaste, event::EnableBracketedPaste};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::telemetry::spawn_member_monitors;
use crate::wallet::{JoinSource, WalletRuntime};

#[derive(Debug, Parser)]
#[command(name = "fed-cdk-tui", version, about)]
struct Cli {
    /// Wallet data directory. Defaults to ~/.fed-cdk-tui.
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Join from a federation-public.json file before opening the TUI.
    #[arg(long, conflicts_with_all = ["invite", "mint_url"])]
    federation_config: Option<PathBuf>,

    /// Join from a cashufed federation invite before opening the TUI.
    #[arg(long, conflicts_with_all = ["federation_config", "mint_url"])]
    invite: Option<String>,

    /// Join by discovering the federation config from any public member URL.
    #[arg(long, conflicts_with_all = ["federation_config", "invite"])]
    mint_url: Option<MintUrl>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Permanently delete this wallet's seed, database, config, and activity data.
    Reset {
        /// Skip the interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            cursor::Hide
        )?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            cursor::Show,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn")
        .with_writer(io::stderr)
        .init();

    let cli = Cli::parse();
    let data_dir = cli.data_dir.unwrap_or_else(default_data_dir);

    if let Some(Command::Reset { yes }) = cli.command {
        return reset_wallet(&data_dir, yes);
    }

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("fed-cdk-tui requires an interactive terminal");
    }

    fs::create_dir_all(&data_dir)
        .with_context(|| format!("could not create {}", data_dir.display()))?;

    let join_source = match (cli.federation_config, cli.invite, cli.mint_url) {
        (Some(path), None, None) => Some(JoinSource::ConfigPath(path)),
        (None, Some(invite), None) => Some(JoinSource::Invite(invite)),
        (None, None, Some(mint_url)) => Some(JoinSource::MintUrl(mint_url)),
        (None, None, None) => None,
        _ => unreachable!("clap rejects conflicting join sources"),
    };

    let runtime = match join_source {
        Some(source) => Some(Arc::new(WalletRuntime::join(&data_dir, source).await?)),
        None => WalletRuntime::load(&data_dir).await?.map(Arc::new),
    };

    run_tui(data_dir, runtime).await
}

fn default_data_dir() -> PathBuf {
    home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".fed-cdk-tui")
}

fn reset_wallet(data_dir: &PathBuf, yes: bool) -> Result<()> {
    if !data_dir.exists() {
        println!("No wallet data exists at {}", data_dir.display());
        return Ok(());
    }

    if !yes {
        println!(
            "This permanently deletes the wallet seed and all eCash at {}.\nType DELETE to continue:",
            data_dir.display()
        );
        let mut confirmation = String::new();
        io::stdin().read_line(&mut confirmation)?;
        if confirmation.trim() != "DELETE" {
            println!("Reset cancelled");
            return Ok(());
        }
    }

    fs::remove_dir_all(data_dir)
        .with_context(|| format!("could not delete {}", data_dir.display()))?;
    println!("Deleted wallet data at {}", data_dir.display());
    Ok(())
}

async fn run_tui(data_dir: PathBuf, runtime: Option<Arc<WalletRuntime>>) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let input_running = Arc::new(AtomicBool::new(true));
    let input_handle = spawn_terminal_input(event_tx.clone(), input_running.clone());

    let mut monitor_generation = 0_u64;
    let mut app = App::new(data_dir, runtime.clone());
    if let Some(runtime) = runtime {
        monitor_generation += 1;
        spawn_member_monitors(
            runtime.public_config(),
            event_tx.clone(),
            monitor_generation,
        );
        wallet::spawn_balance_refresh(runtime, event_tx.clone());
    }

    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let result = loop {
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        tokio::select! {
            _ = tick.tick() => app.tick(),
            maybe_event = event_rx.recv() => {
                let Some(event) = maybe_event else {
                    break Ok(());
                };
                match event {
                    AppEvent::Terminal(event) => {
                        let commands = app.handle_terminal_event(event);
                        for command in commands {
                            execute_app_command(command, &mut app, event_tx.clone()).await;
                            if app.should_quit() {
                                break;
                            }
                        }
                        if app.should_quit() {
                            break Ok(());
                        }
                    }
                    AppEvent::Joined(result) => {
                        if let Ok(runtime) = &result {
                            monitor_generation += 1;
                            spawn_member_monitors(
                                runtime.public_config(),
                                event_tx.clone(),
                                monitor_generation,
                            );
                            wallet::spawn_balance_refresh(runtime.clone(), event_tx.clone());
                        }
                        app.apply_event(AppEvent::Joined(result));
                    }
                    event => app.apply_event(event),
                }
            }
        }
    };

    input_running.store(false, Ordering::Relaxed);
    let _ = input_handle.join();
    result
}

fn spawn_terminal_input(
    tx: mpsc::UnboundedSender<AppEvent>,
    running: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if tx.send(AppEvent::Terminal(event)).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                },
                Ok(false) => {}
                Err(_) => return,
            }
        }
    })
}

async fn execute_app_command(
    command: AppCommand,
    app: &mut App,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    match command {
        AppCommand::Quit => app.request_quit(),
        AppCommand::Join(source) => {
            app.operation_started("join", "Validating federation roster");
            let data_dir = app.data_dir().to_path_buf();
            tokio::spawn(async move {
                let result = WalletRuntime::join(&data_dir, source)
                    .await
                    .map(Arc::new)
                    .map_err(|err| err.to_string());
                let _ = tx.send(AppEvent::Joined(result));
            });
        }
        AppCommand::Mint(amount) => {
            if let Some(runtime) = app.runtime() {
                app.operation_started("mint", &format!("Requesting {amount} sat quote"));
                wallet::spawn_mint(runtime, amount, tx);
            }
        }
        AppCommand::Melt(invoice) => {
            if let Some(runtime) = app.runtime() {
                app.operation_started("melt", "Requesting payment quote");
                wallet::spawn_melt(runtime, invoice, tx);
            }
        }
        AppCommand::Send(amount) => {
            if let Some(runtime) = app.runtime() {
                app.operation_started("send", &format!("Preparing {amount} sat token"));
                wallet::spawn_send(runtime, amount, tx);
            }
        }
        AppCommand::Receive(token) => {
            if let Some(runtime) = app.runtime() {
                app.operation_started("receive", "Validating and swapping incoming token");
                wallet::spawn_receive(runtime, token, tx);
            }
        }
    }
}
