//! CLI for the Nitro binding generator.

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use uniffi_bindgen_nitro::{generate_hybrids, generate_spec, load};

/// Generate React Native Nitro bindings from a UniFFI-enabled cdylib.
#[derive(Debug, Parser)]
#[command(name = "uniffi-bindgen-nitro", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Emit the Nitro TypeScript spec and the C++ that crosses the UniFFI ABI.
    Spec {
        /// Built cdylib to read metadata from.
        #[arg(long)]
        library: Utf8PathBuf,
        /// Crate to generate for, when the library holds more than one.
        #[arg(long = "crate")]
        crate_name: Option<String>,
        /// The crate's `uniffi.toml`.
        #[arg(long)]
        config: Option<Utf8PathBuf>,
        /// Directory for the generated `.nitro.ts` files.
        #[arg(long)]
        ts_out: Utf8PathBuf,
        /// Directory for the generated C++.
        #[arg(long)]
        cpp_out: Utf8PathBuf,
        /// Directory for the generated Node test harness.
        #[arg(long)]
        node_out: Option<Utf8PathBuf>,
    },
    /// Emit the Nitro HybridObject implementations, after nitrogen has run.
    Hybrids {
        /// Built cdylib to read metadata from.
        #[arg(long)]
        library: Utf8PathBuf,
        /// Crate to generate for, when the library holds more than one.
        #[arg(long = "crate")]
        crate_name: Option<String>,
        /// The crate's `uniffi.toml`.
        #[arg(long)]
        config: Option<Utf8PathBuf>,
        /// The package's `nitrogen` directory.
        #[arg(long)]
        nitrogen: Utf8PathBuf,
        /// Directory for the generated C++.
        #[arg(long)]
        cpp_out: Utf8PathBuf,
    },
}

fn main() -> std::process::ExitCode {
    if let Err(error) = run() {
        eprintln!("uniffi-bindgen-nitro: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

fn run() -> uniffi_bindgen_nitro::Result<()> {
    match Cli::parse().command {
        Command::Spec {
            library,
            crate_name,
            config,
            ts_out,
            cpp_out,
            node_out,
        } => {
            let (ci, nitro_config) = load(&library, crate_name.as_deref(), config.as_deref())?;
            let files = generate_spec(&ci, &nitro_config)?;
            for file in files.get("ts").into_iter().flatten() {
                println!("{}", file.write(&ts_out)?);
            }
            for file in files.get("cpp").into_iter().flatten() {
                println!("{}", file.write(&cpp_out)?);
            }
            if let Some(node_out) = node_out {
                for file in files.get("node").into_iter().flatten() {
                    println!("{}", file.write(&node_out)?);
                }
            }
        }
        Command::Hybrids {
            library,
            crate_name,
            config,
            nitrogen,
            cpp_out,
        } => {
            let (ci, nitro_config) = load(&library, crate_name.as_deref(), config.as_deref())?;
            for file in generate_hybrids(&ci, &nitro_config, &nitrogen)? {
                println!("{}", file.write(&cpp_out)?);
            }
        }
    }
    Ok(())
}
