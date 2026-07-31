use std::path::Path;

use anyhow::{anyhow, Result};

use crate::config::{DatabaseEngine, Settings};

/// Route migration to the appropriate database backend module
pub async fn run_migration(
    _work_dir: &Path,
    settings: &Settings,
    nutshell_db: &str,
    _db_password: Option<String>,
    verify_only: bool,
) -> Result<()> {
    tracing::info!("Starting nutshell database migration routing...");

    match settings.database.engine {
        #[cfg(feature = "sqlite")]
        DatabaseEngine::Sqlite => {
            if nutshell_db.starts_with("postgres://") || nutshell_db.starts_with("postgresql://") {
                anyhow::bail!(
                    "Cross-backend Nutshell migration is not supported: configure a PostgreSQL CDK target for a PostgreSQL source"
                );
            }
            let sql_db_path = _work_dir.join("cdk-mintd.sqlite");
            if verify_only {
                cdk_sqlite::mint::migrate::verify_nutshell_migration(
                    &sql_db_path,
                    nutshell_db,
                    _db_password.as_deref(),
                )
                .map_err(|e| anyhow!(e))?;
            } else {
                cdk_sqlite::mint::migrate::migrate_from_nutshell(
                    &sql_db_path,
                    nutshell_db,
                    _db_password,
                )
                .await
                .map_err(|e| anyhow!(e))?;
            }
        }
        #[cfg(feature = "postgres")]
        DatabaseEngine::Postgres => {
            if !nutshell_db.starts_with("postgres://") && !nutshell_db.starts_with("postgresql://")
            {
                anyhow::bail!(
                    "Cross-backend Nutshell migration is not supported: configure a SQLite CDK target for a SQLite source"
                );
            }
            let pg_config = settings.database.postgres.as_ref().ok_or_else(|| {
                anyhow!("PostgreSQL configuration is required when using PostgreSQL engine")
            })?;
            if verify_only {
                cdk_postgres::migrate::verify_nutshell_migration(
                    &pg_config.url,
                    pg_config.tls_mode.as_deref(),
                    nutshell_db,
                )
                .await
                .map_err(|e| anyhow!(e))?;
            } else {
                cdk_postgres::migrate::migrate_from_nutshell(
                    &pg_config.url,
                    pg_config.tls_mode.as_deref(),
                    nutshell_db,
                )
                .await
                .map_err(|e| anyhow!(e))?;
            }
        }
        #[cfg(not(feature = "sqlite"))]
        DatabaseEngine::Sqlite => {
            anyhow::bail!("SQLite support not compiled in.");
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseEngine::Postgres => {
            anyhow::bail!("PostgreSQL support not compiled in.");
        }
    }

    Ok(())
}
