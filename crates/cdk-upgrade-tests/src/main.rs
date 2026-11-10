//! Released-to-current mint and wallet compatibility test orchestrator.
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use serde_json::json;

mod configuration;
mod process;
mod repository;
mod scenario;

use self::scenario::Harness;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Check(String),
}

type Result<T> = std::result::Result<T, Error>;

fn check<M>(condition: bool, message: M) -> Result<()>
where
    M: Into<String>,
{
    match condition {
        true => Ok(()),
        false => Err(Error::Check(message.into())),
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Case {
    Normal,
    ConfigDefaults,
    ConfigRich,
    ShortSeed,
    Metadata,
    ShortSeedMetadata,
}

impl Case {
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::ConfigDefaults => "config-defaults",
            Self::ConfigRich => "config-rich",
            Self::ShortSeed => "short-seed",
            Self::Metadata => "metadata",
            Self::ShortSeedMetadata => "short-seed-metadata",
        }
    }

    fn short_seed(self) -> bool {
        matches!(self, Self::ShortSeed | Self::ShortSeedMetadata)
    }

    fn custom_metadata(self) -> bool {
        matches!(self, Self::Metadata | Self::ShortSeedMetadata)
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Order {
    MintFirst,
    WalletFirst,
}

impl Order {
    fn name(self) -> &'static str {
        match self {
            Self::MintFirst => "mint-first",
            Self::WalletFirst => "wallet-first",
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Exercise released mint/wallet databases across an upgrade")]
struct Args {
    #[arg(long = "from", default_value = "v0.17.7")]
    baseline: String,
    #[arg(long, default_value = "25", value_parser = clap::value_parser!(u32).range(1..))]
    rounds: u32,
    #[arg(long, value_enum, num_args = 1.., default_values = ["normal", "short-seed", "metadata", "short-seed-metadata", "config-defaults", "config-rich"])]
    cases: Vec<Case>,
    #[arg(long, value_enum, num_args = 1.., default_values = ["mint-first", "wallet-first"])]
    orders: Vec<Order>,
    #[arg(long)]
    scratch_root: Option<PathBuf>,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate directory")
        .parent()
        .expect("workspace directory")
        .to_path_buf()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal?;
            Err(Error::Check("interrupted; scratch artifacts preserved".to_owned()))
        }
        result = run_suite(args) => result,
    }
}

async fn run_suite(args: Args) -> Result<()> {
    let scratch = args
        .scratch_root
        .unwrap_or_else(|| match Path::new("/data/rust/tmp").is_dir() {
            true => PathBuf::from("/data/rust/tmp"),
            false => std::env::temp_dir(),
        });
    let root = scratch.join(format!("cdk-upgrade-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    println!("Artifacts: {}", root.display());
    let workspace = workspace();
    repository::export_release(&workspace, &args.baseline, &root.join("old-source"))?;
    repository::export_current(&workspace, &root.join("new-source"))?;
    repository::build(&workspace, &root.join("old-source"), &root.join("old"))?;
    repository::build(&workspace, &root.join("new-source"), &root.join("new"))?;
    let harness = Harness::new(root.clone(), args.rounds)?;
    let rounds = u64::from(args.rounds);
    let mut report = json!({
        "baseline": args.baseline,
        "rounds": args.rounds,
        "planned_per_successful_scenario": {
            "workload_rounds": 4 * rounds,
            "completed_money_operations": 24 * rounds + 9,
            "quotes_created": 8 * rounds + 9,
            "double_spend_checks": 4 * rounds,
            "quote_replay_checks": 2,
            "pending_send_reconciliations": 1,
            "seed_restores": 1,
            "pending_upgrade_checks": 2,
            "prepared_send_recoveries": 1,
            "prepared_melt_recoveries": 1,
            "pending_melt_paid_recoveries": 1,
            "pending_melt_failed_recoveries": 1,
            "unpaid_quote_completions": 1,
        },
        "scenarios": [],
    });
    let mut failures = Vec::new();
    for case in args.cases {
        for order in &args.orders {
            let mut result = harness.scenario(case, *order).await;
            let operations_path = root
                .join(format!("{}-{}", case.name(), order.name()))
                .join("operations.json");
            let operations: Option<serde_json::Value> = match operations_path.exists() {
                true => Some(serde_json::from_slice(&fs::read(operations_path)?)?),
                false => None,
            };
            if result.is_ok() {
                result = check(
                    operations.as_ref() == Some(&report["planned_per_successful_scenario"]),
                    "completed operation counts did not match the planned workload",
                );
            }
            let mut entry =
                json!({"case": case.name(), "order": order.name(), "passed": result.is_ok()});
            match result {
                Ok(()) => println!("PASSED: {}/{}", case.name(), order.name()),
                Err(error) => {
                    let message = format!("{}/{}: {error}", case.name(), order.name());
                    println!("FAILED: {message}");
                    entry["error"] = json!(error.to_string());
                    failures.push(message);
                }
            }
            if let Some(operations) = operations {
                entry["operations"] = operations;
            }
            report["scenarios"]
                .as_array_mut()
                .expect("scenario list")
                .push(entry);
            fs::write(
                root.join("report.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
        }
    }
    check(
        failures.is_empty(),
        format!(
            "Upgrade failures (artifacts preserved at {}):\n{}",
            root.display(),
            failures.join("\n")
        ),
    )?;
    fs::remove_dir_all(root)?;
    println!("All mint and wallet upgrade scenarios passed.");
    Ok(())
}
