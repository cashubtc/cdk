//! Configuration matrix and the mint-first / wallet-first upgrade boundaries.
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::configuration;
use super::process::{run, Process};
use super::{check, Case, Order, Result};

pub(crate) const PUBKEY: &str =
    "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
pub(crate) const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[derive(Debug)]
pub(crate) struct Harness {
    root: PathBuf,
    rounds: u32,
    client: reqwest::Client,
}

impl Harness {
    pub(crate) fn new(root: PathBuf, rounds: u32) -> Result<Self> {
        Ok(Self {
            root,
            rounds,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()?,
        })
    }

    fn command(&self, binary: &Path, stale: bool) -> Command {
        let mut command = Command::new(binary);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CDK_MINTD_") {
                command.env_remove(key);
            }
        }
        command
            .env(
                "CDK_MINTD_MINT_NAME",
                match stale {
                    false => "name from legacy environment",
                    true => "must not override persisted configuration",
                },
            )
            .env(
                "CDK_MINTD_QUOTE_TTL_MINT",
                match stale {
                    false => "7200",
                    true => "1",
                },
            );
        if stale {
            command
                .env("CDK_MINTD_MINT_DESCRIPTION", "stale description")
                .env("CDK_MINTD_LN_MAX_MINT", "1")
                .env("CDK_MINTD_QUOTE_TTL_MELT", "1");
        }
        command
    }

    async fn get(&self, url: &str, route: &str) -> Result<Value> {
        Ok(self
            .client
            .get(format!("{url}{route}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn start(
        &self,
        binary: &Path,
        directory: &Path,
        url: &str,
        stale: bool,
        config: Option<&Path>,
    ) -> Result<Process> {
        let log = directory.join(format!(
            "mint-{}.log",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let stream = fs::File::create(&log)?;
        let mut command = self.command(binary, stale);
        command.env("CDK_UPGRADE_PAYMENT_CONTROL", directory.join("payments"));
        command
            .arg("--work-dir")
            .arg(directory)
            .stdout(Stdio::from(stream.try_clone()?))
            .stderr(Stdio::from(stream));
        if let Some(config) = config {
            command.arg("--config").arg(config);
        }
        let mut process = Process::spawn(&mut command)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            check(
                process.status()?.is_none(),
                format!(
                    "mint exited during startup; see {}\n{}",
                    log.display(),
                    fs::read_to_string(&log)?
                ),
            )?;
            if self.get(url, "/v1/info").await.is_ok() {
                return Ok(process);
            }
            check(
                Instant::now() < deadline,
                format!("mint readiness timed out; see {}", log.display()),
            )?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn configure(
        &self,
        binary: &Path,
        directory: &Path,
        legacy: &Path,
        existing: bool,
    ) -> Result<PathBuf> {
        let migrated = directory.join("import.toml");
        run(self
            .command(binary, false)
            .args(["config", "migrate", "--file"])
            .arg(legacy)
            .arg("--output")
            .arg(&migrated))?;
        configuration::verify_migration(legacy, &migrated)?;
        run(self
            .command(binary, false)
            .arg("--work-dir")
            .arg(directory)
            .args([
                "config",
                "init",
                match existing {
                    true => "--existing-mint",
                    false => "--new-mint",
                },
                "--file",
            ])
            .arg(&migrated))?;
        Ok(migrated)
    }

    async fn wallet(
        &self,
        side: &str,
        phase: &str,
        directory: &Path,
        url: &str,
        rounds: u32,
        stale: bool,
    ) -> Result<()> {
        println!("Wallet {side}: {phase}, {rounds} rounds");
        let mut child = Process::spawn(
            self.command(&self.root.join(side).join("wallet"), stale)
                .arg(phase)
                .arg(directory)
                .arg(url)
                .arg(rounds.to_string()),
        )?;
        check(
            child
                .wait(Duration::from_secs(120_u64.max(u64::from(rounds) * 45)))
                .await?
                .success(),
            format!("{side} wallet failed in {phase}"),
        )
    }

    fn export_config(&self, binary: &Path, directory: &Path, name: &str) -> Result<toml::Value> {
        let path = directory.join(name);
        run(self
            .command(binary, true)
            .arg("--work-dir")
            .arg(directory)
            .args(["config", "export", "--file"])
            .arg(&path))?;
        Ok(toml::from_str(&fs::read_to_string(path)?)?)
    }

    async fn check_config_updates(&self, binary: &Path, directory: &Path) -> Result<()> {
        let before = self.export_config(binary, directory, "config-before.toml")?;
        let migrated: toml::Value =
            toml::from_str(&fs::read_to_string(directory.join("import.toml"))?)?;
        configuration::contains(&migrated, Some(&before), "persisted config")?;
        run(self
            .command(binary, true)
            .arg("--work-dir")
            .arg(directory)
            .args(["config", "apply", "--file"])
            .arg(directory.join("config-before.toml")))?;
        let after = self.export_config(binary, directory, "config-noop.toml")?;
        check(
            before == after,
            "no-op config apply changed persisted settings",
        )?;
        let mut invalid = before.clone();
        let info = invalid["info"].as_table_mut().expect("info section");
        info.remove("mnemonic");
        let secret = directory.join("different-signing-seed");
        fs::write(
            &secret,
            "a different signing seed that is longer than thirty two bytes",
        )?;
        info.insert(
            "seed".to_owned(),
            toml::Value::String(format!("file:{}", secret.display())),
        );
        let candidate = directory.join("invalid-identity.toml");
        configuration::write_document(&candidate, &invalid)?;
        let output = self
            .command(binary, true)
            .arg("--work-dir")
            .arg(directory)
            .args(["config", "apply", "--file"])
            .arg(candidate)
            .output()?;
        check(
            !output.status.success(),
            "config apply accepted changed signing identity",
        )?;
        check(
            String::from_utf8_lossy(&output.stderr).contains("signing identity"),
            "identity fixture failed for an unrelated reason",
        )?;
        check(
            before == self.export_config(binary, directory, "config-rejected.toml")?,
            "rejected config apply changed persisted settings",
        )
    }

    async fn same_mint(&self, url: &str, before: &Value) -> Result<()> {
        let info = self.get(url, "/v1/info").await?;
        for field in [
            "pubkey",
            "name",
            "description",
            "description_long",
            "motd",
            "icon_url",
            "contact",
            "tos_url",
        ] {
            check(
                info[field] == before["info"][field],
                format!("mint metadata changed: {field}"),
            )?;
        }
        // Other NUT capabilities may legitimately be added between releases.
        // Preserve the configured BOLT11 limits.
        for nut in ["4", "5"] {
            check(
                info["nuts"][nut]["disabled"] == before["info"]["nuts"][nut]["disabled"],
                format!("payment configuration changed: nut {nut}, disabled"),
            )?;
            let find = |settings: &Value| {
                settings["methods"]
                    .as_array()
                    .and_then(|methods| {
                        methods
                            .iter()
                            .find(|method| method["method"] == "bolt11" && method["unit"] == "sat")
                    })
                    .cloned()
            };
            let old = find(&before["info"]["nuts"][nut]);
            let new = find(&info["nuts"][nut]);
            check(
                old.is_some() && new.is_some(),
                "configured BOLT11 method disappeared",
            )?;
            for field in ["min_amount", "max_amount"] {
                check(
                    old.as_ref().expect("old method")[field]
                        == new.as_ref().expect("new method")[field],
                    format!("payment configuration changed: nut {nut}, {field}"),
                )?;
            }
        }
        check(
            self.get(url, "/v1/keys").await? == before["keys"],
            "signing public keys changed",
        )?;
        check(
            self.get(url, "/v1/keysets").await? == before["keysets"],
            "keysets or fees changed",
        )
    }

    pub(crate) async fn scenario(&self, case: Case, order: Order) -> Result<()> {
        println!("\nTesting {}, {}", case.name(), order.name());
        let directory = self.root.join(format!("{}-{}", case.name(), order.name()));
        fs::create_dir(&directory)?;
        fs::create_dir(directory.join("payments"))?;
        if matches!(case, Case::ConfigDefaults) {
            fs::write(
                directory.join("wallet-defaults"),
                b"omit optional wallet settings",
            )?;
        }
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        let url = format!("http://127.0.0.1:{port}");
        let legacy = directory.join("legacy.toml");
        fs::write(&legacy, configuration::legacy_document(&url, port, case)?)?;
        let old = self.root.join("old/mintd");
        let new = self.root.join("new/mintd");
        let database_config =
            fs::read_to_string(self.root.join("old-source/crates/cdk-mintd/src/cli.rs"))?
                .contains("pub enum ConfigCommands");
        if database_config {
            self.configure(&old, &directory, &legacy, false)?;
        }
        let process = self
            .start(
                &old,
                &directory,
                &url,
                false,
                match database_config {
                    true => None,
                    false => Some(legacy.as_path()),
                },
            )
            .await?;
        let mut before = json!({
            "info": self.get(&url, "/v1/info").await?,
            "keys": self.get(&url, "/v1/keys").await?,
            "keysets": self.get(&url, "/v1/keysets").await?,
        });
        fs::write(
            directory.join("mint-before.json"),
            serde_json::to_vec_pretty(&before)?,
        )?;
        check(
            before["info"]["name"] == "name from legacy environment",
            "legacy name override was not applied",
        )?;
        if case.custom_metadata() {
            check(
                before["info"]["pubkey"] == PUBKEY,
                "fixture did not establish custom metadata pubkey",
            )?;
        }
        self.wallet("old", "seed", &directory, &url, self.rounds, false)
            .await?;
        let quote = fs::read_to_string(directory.join("unclaimed-quote.txt"))?;
        check(
            self.get(&url, &format!("/v1/mint/quote/bolt11/{quote}"))
                .await?["state"]
                == "PAID",
            "fixture quote must be paid before shutdown",
        )?;
        self.wallet("old", "seed-pending", &directory, &url, 0, false)
            .await?;
        if matches!(order, Order::WalletFirst) {
            self.wallet("new", "verify-pending", &directory, &url, 0, false)
                .await?;
            self.wallet("new", "exercise", &directory, &url, self.rounds, false)
                .await?;
        }
        process.stop().await?;
        let imported = match database_config {
            true => directory.join("import.toml"),
            false => self.configure(&new, &directory, &legacy, true)?,
        };
        let document: toml::Value = toml::from_str(&fs::read_to_string(&imported)?)?;
        check(
            document["info"]["quote_ttl"]["mint_ttl"].as_integer() == Some(7200),
            "migrated quote TTL changed",
        )?;
        check(
            document["mint_info"]["name"].as_str() == Some("name from legacy environment"),
            "migrated mint name changed",
        )?;
        let process = self.start(&new, &directory, &url, true, None).await?;
        self.same_mint(&url, &before).await?;
        if matches!(order, Order::MintFirst) {
            self.wallet("old", "verify-pending", &directory, &url, 0, true)
                .await?;
            self.wallet("old", "exercise", &directory, &url, self.rounds, true)
                .await?;
        }
        self.wallet("new", "verify-pending", &directory, &url, 0, true)
            .await?;
        fs::write(directory.join("payments/release-outgoing"), b"release")?;
        fs::write(directory.join("payments/release-incoming"), b"release")?;
        self.wallet("new", "finish-pending", &directory, &url, 0, true)
            .await?;
        self.wallet("new", "finish", &directory, &url, self.rounds, true)
            .await?;
        process.stop().await?;
        self.check_config_updates(&new, &directory).await?;
        let applied = directory.join("applied.toml");
        fs::write(
            &applied,
            fs::read_to_string(imported)?
                .replace("motd = \"before upgrade\"", "motd = \"after upgrade\""),
        )?;
        run(self
            .command(&new, true)
            .arg("--work-dir")
            .arg(&directory)
            .args(["config", "apply", "--file"])
            .arg(applied))?;
        for name in ["alice", "bob"] {
            let path = directory.join(format!("{name}.json"));
            let mut config: Value = serde_json::from_slice(&fs::read(&path)?)?;
            config["target_proof_count"] = json!(8);
            fs::write(path, serde_json::to_vec(&config)?)?;
        }
        let process = self.start(&new, &directory, &url, true, None).await?;
        before["info"]["motd"] = json!("after upgrade");
        self.same_mint(&url, &before).await?;
        self.wallet("new", "verify", &directory, &url, 0, true)
            .await?;
        self.wallet("new", "exercise", &directory, &url, self.rounds, true)
            .await?;
        self.wallet("new", "restore", &directory, &url, 0, true)
            .await?;
        process.stop().await?;
        let applied: toml::Value =
            toml::from_str(&fs::read_to_string(directory.join("applied.toml"))?)?;
        let persisted = self.export_config(&new, &directory, "config-final.toml")?;
        configuration::contains(&applied, Some(&persisted), "config after restart")?;
        Ok(())
    }
}
