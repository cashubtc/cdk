//! Historical configuration fixtures and migration contracts.
use std::fs;
use std::path::Path;

use super::scenario::{MNEMONIC, PUBKEY};
use super::{check, Case, Error, Result};

pub(crate) fn write_document(path: &Path, value: &toml::Value) -> Result<()> {
    fs::write(
        path,
        toml::to_string(value).map_err(|error| Error::Check(error.to_string()))?,
    )?;
    Ok(())
}

pub(crate) fn legacy_document(url: &str, port: u16, case: Case) -> Result<String> {
    let secret = match case.short_seed() {
        true => "seed = \"legacy\"".to_owned(),
        false => format!("mnemonic = \"{MNEMONIC}\""),
    };
    let metadata = match case.custom_metadata() {
        true => format!("pubkey = \"{PUBKEY}\""),
        false => String::new(),
    };
    let template = include_str!("../../cdk-integration-tests/upgrade/fixtures/v0.17.toml")
        .replace("{url}", url)
        .replace("{port}", &port.to_string())
        .replace("{secret}", &secret)
        .replace("{metadata}", &metadata);
    let mut document: toml::Value = toml::from_str(&template)?;
    match case {
        Case::ConfigDefaults => {
            let info = document["info"].as_table_mut().expect("info table");
            info.remove("input_fee_ppk");
            info.remove("use_keyset_v2");
        }
        Case::ConfigRich => {
            document["mint_info"]
                .as_table_mut()
                .expect("metadata table")
                .extend([
                    (
                        "description_long".to_owned(),
                        "Historical long description".into(),
                    ),
                    ("icon_url".to_owned(), "https://example.com/icon.png".into()),
                    ("contact_email".to_owned(), "mint@example.com".into()),
                    ("tos_url".to_owned(), "https://example.com/terms".into()),
                ]);
            document["info"]["quote_ttl"]["melt_ttl"] = 1800.into();
            let info = document["info"].as_table_mut().expect("info table");
            info.insert("enable_info_page".to_owned(), false.into());
            info.insert(
                "http_cache".to_owned(),
                toml::Value::Table(toml::Table::from_iter([
                    ("backend".to_owned(), "memory".into()),
                    ("ttl".to_owned(), 37.into()),
                    ("tti".to_owned(), 19.into()),
                ])),
            );
            document
                .as_table_mut()
                .expect("configuration table")
                .insert(
                    "limits".to_owned(),
                    toml::Value::Table(toml::Table::from_iter([
                        ("max_inputs".to_owned(), 512.into()),
                        ("max_outputs".to_owned(), 256.into()),
                    ])),
                );
            document["ln"]["min_mint"] = 2.into();
            document["ln"]["max_mint"] = 4096.into();
            document["ln"]["min_melt"] = 2.into();
            document["ln"]["max_melt"] = 64.into();
        }
        _ => {}
    }
    toml::to_string(&document).map_err(|error| Error::Check(error.to_string()))
}

// Compare every historical leaf, accepting additional fields in the new schema.
// Explicitly renamed fields belong in the mapping below, so silent drops fail.
pub(crate) fn contains(
    expected: &toml::Value,
    actual: Option<&toml::Value>,
    path: &str,
) -> Result<()> {
    match expected {
        toml::Value::Table(table) => {
            for (name, value) in table {
                contains(
                    value,
                    actual.and_then(|value| value.get(name)),
                    &format!("{path}.{name}"),
                )?;
            }
            Ok(())
        }
        _ => check(
            actual == Some(expected),
            format!("configuration changed or disappeared: {path}"),
        ),
    }
}

pub(crate) fn verify_migration(legacy: &Path, migrated: &Path) -> Result<()> {
    let mut expected: toml::Value = toml::from_str(&fs::read_to_string(legacy)?)?;
    let actual: toml::Value = toml::from_str(&fs::read_to_string(migrated)?)?;
    // Migration absorbs operational environment overrides into the document.
    expected["mint_info"]["name"] = "name from legacy environment".into();
    expected["info"]["quote_ttl"]["mint_ttl"] = 7200.into();
    for name in ["seed", "mnemonic"] {
        if let Some(secret) = expected["info"]
            .as_table_mut()
            .expect("info table")
            .remove(name)
        {
            let reference = actual["info"].get(name).and_then(toml::Value::as_str);
            let file = reference.and_then(|value| value.strip_prefix("file:"));
            check(
                file.is_some(),
                format!("{name} was not migrated to a secret reference"),
            )?;
            check(
                fs::read_to_string(file.expect("checked reference"))?.trim()
                    == secret.as_str().expect("secret string"),
                format!("migration changed signing secret: {name}"),
            )?;
        }
    }
    for (section, value) in expected.as_table().expect("configuration table") {
        if section != "ln" {
            contains(value, actual.get(section), section)?;
        }
    }
    let mut backend = expected["ln"].clone();
    let table = backend.as_table_mut().expect("backend table");
    let name = table.remove("ln_backend").expect("backend name");
    table.insert("backend".to_owned(), name);
    table.insert("unit".to_owned(), "sat".into());
    contains(
        &backend,
        actual.get("payment_backend").and_then(|value| value.get(0)),
        "payment_backend.0",
    )?;
    // Pin defaults omitted by the legacy fixture rather than borrowing the
    // current implementation's defaults (which may be the regression).
    if expected.get("limits").is_none() {
        for field in ["max_inputs", "max_outputs"] {
            check(
                actual
                    .get("limits")
                    .and_then(|limits| limits.get(field))
                    .and_then(toml::Value::as_integer)
                    == Some(1000),
                format!("historical default changed: limits.{field}"),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_configuration_profiles_render() {
        for case in [
            Case::Normal,
            Case::ShortSeed,
            Case::Metadata,
            Case::ShortSeedMetadata,
            Case::ConfigDefaults,
            Case::ConfigRich,
        ] {
            let document = legacy_document("http://127.0.0.1:8085", 8085, case)
                .expect("profile must render without missing-table panics");
            let parsed: toml::Value = toml::from_str(&document).expect("valid legacy TOML");
            assert_eq!(parsed["info"]["listen_port"].as_integer(), Some(8085));
        }
    }

    #[test]
    fn rejects_dropped_renamed_and_changed_historical_settings() {
        let expected: toml::Value =
            toml::from_str("[info.quote_ttl]\nmelt_ttl = 1800").expect("historical setting");
        for document in [
            "[info]",
            "[info.quote_ttl]\nrenamed_melt_ttl = 1800",
            "[info.quote_ttl]\nmelt_ttl = 120",
        ] {
            let actual: toml::Value = toml::from_str(document).expect("candidate document");
            let error = contains(&expected, Some(&actual), "config")
                .expect_err("historical TTL must survive unchanged");
            assert!(error.to_string().contains("config.info.quote_ttl.melt_ttl"));
        }
    }

    #[test]
    fn permits_new_settings_without_weakening_historical_assertions() {
        let expected: toml::Value =
            toml::from_str("[limits]\nmax_inputs = 512").expect("historical limits");
        let actual: toml::Value =
            toml::from_str("new_feature = true\n[limits]\nmax_inputs = 512\nmax_outputs = 256")
                .expect("extended settings");
        contains(&expected, Some(&actual), "config").expect("new fields are compatible");
    }
}
