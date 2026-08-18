//! DNSSEC-validated TXT resolution for BIP353.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dnssec_prover::query::build_txt_proof_async;
use dnssec_prover::rr::{Name, RR};
use dnssec_prover::ser::parse_rr_stream;
use dnssec_prover::validation::verify_rr_stream;

use crate::HttpError;

const GOOGLE_DNS_RESOLVERS: [SocketAddr; 2] = [
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53),
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)), 53),
];
const DNS_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn name_from_domain(domain: &str) -> Result<Name, HttpError> {
    let absolute_name = match domain.ends_with('.') {
        true => domain.to_owned(),
        false => format!("{domain}."),
    };

    Name::try_from(absolute_name)
        .map_err(|()| HttpError::Other(format!("Invalid domain name: {domain}")))
}

pub(crate) async fn resolve_dns_txt(domain: &str) -> Result<Vec<String>, HttpError> {
    let name = name_from_domain(domain)?;
    let mut last_error = "No DNS resolvers configured".to_owned();

    for resolver in GOOGLE_DNS_RESOLVERS {
        match tokio::time::timeout(DNS_QUERY_TIMEOUT, build_txt_proof_async(resolver, &name)).await
        {
            Ok(Ok((proof, _ttl))) => return validated_txt_records(&name, &proof),
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => last_error = format!("Query to {resolver} timed out"),
        }
    }

    Err(HttpError::Other(format!(
        "Failed to build DNSSEC proof: {last_error}"
    )))
}

pub(crate) fn validated_txt_records(name: &Name, proof: &[u8]) -> Result<Vec<String>, HttpError> {
    let records = parse_rr_stream(proof)
        .map_err(|()| HttpError::Other("DNS resolver returned an invalid proof".to_owned()))?;
    let verified = verify_rr_stream(&records)
        .map_err(|_| HttpError::Other("DNSSEC signature validation failed".to_owned()))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| HttpError::Other(format!("Invalid system clock: {error}")))?
        .as_secs();
    validate_proof_time(verified.valid_from, verified.expires, now)?;

    collect_txt_records(verified.resolve_name(name))
}

fn validate_proof_time(valid_from: u64, expires: u64, now: u64) -> Result<(), HttpError> {
    if now < valid_from {
        return Err(HttpError::Other(
            "DNSSEC records are not yet valid; check the system clock".to_owned(),
        ));
    }
    if now > expires {
        return Err(HttpError::Other(
            "DNSSEC records have expired; check the system clock".to_owned(),
        ));
    }
    Ok(())
}

fn collect_txt_records<'a>(
    records: impl IntoIterator<Item = &'a RR>,
) -> Result<Vec<String>, HttpError> {
    records
        .into_iter()
        .filter_map(|record| match record {
            RR::Txt(txt) => Some(String::from_utf8(txt.data.as_vec()).map_err(|error| {
                HttpError::Other(format!("TXT record is not valid UTF-8: {error}"))
            })),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use dnssec_prover::rr::{Txt, TxtBytes};

    use super::*;

    #[test]
    fn makes_domain_name_absolute() {
        let name =
            name_from_domain("alice.user._bitcoin-payment.example.com").expect("valid domain name");
        assert_eq!(name.as_str(), "alice.user._bitcoin-payment.example.com.");
    }

    #[test]
    fn rejects_invalid_domain_name() {
        assert!(name_from_domain("not a domain").is_err());
    }

    #[test]
    fn extracts_utf8_txt_records() {
        let name =
            Name::try_from("alice.user._bitcoin-payment.example.com.").expect("valid domain name");
        let records = [
            RR::Txt(Txt {
                name: name.clone(),
                data: TxtBytes::try_from("bitcoin:bc1example").expect("valid TXT record"),
            }),
            RR::Txt(Txt {
                name,
                data: TxtBytes::try_from("metadata").expect("valid TXT record"),
            }),
        ];

        let txt_records = collect_txt_records(records.iter()).expect("valid UTF-8 TXT records");

        assert_eq!(txt_records, ["bitcoin:bc1example", "metadata"]);
    }

    #[test]
    fn validates_dnssec_time_window() {
        assert!(validate_proof_time(100, 200, 100).is_ok());
        assert!(validate_proof_time(100, 200, 200).is_ok());
        assert!(validate_proof_time(100, 200, 99).is_err());
        assert!(validate_proof_time(100, 200, 201).is_err());
    }

    #[test]
    fn rejects_malformed_dnssec_proof() {
        let name =
            Name::try_from("alice.user._bitcoin-payment.example.com.").expect("valid domain name");
        assert!(validated_txt_records(&name, b"not a DNSSEC proof").is_err());
    }
}
