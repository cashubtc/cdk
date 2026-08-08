use url::Url;

const INVALID_URL_FOR_LOGS: &str = "[INVALID URL]";

pub(super) fn url_for_logs(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return INVALID_URL_FOR_LOGS.to_owned();
    };

    if url.set_password(None).is_err() || url.set_username("").is_err() {
        return INVALID_URL_FOR_LOGS.to_owned();
    }

    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::{url_for_logs, INVALID_URL_FOR_LOGS};

    #[test]
    fn removes_url_credentials() {
        let url = "https://alice:s3cr3t@example.com:8443/esplora/api?network=main#tip";

        let logged_url = url_for_logs(url);

        assert_eq!(
            logged_url,
            "https://example.com:8443/esplora/api?network=main#tip"
        );
        assert!(!logged_url.contains("alice"));
        assert!(!logged_url.contains("s3cr3t"));
    }

    #[test]
    fn removes_credentials_from_electrum_url() {
        let url = "ssl://alice:s3cr3t@example.com:50002";

        assert_eq!(url_for_logs(url), "ssl://example.com:50002");
    }

    #[test]
    fn preserves_ipv6_port_and_path() {
        let url = "https://user:pass@[2001:db8::1]:3002/api";

        assert_eq!(url_for_logs(url), "https://[2001:db8::1]:3002/api");
    }

    #[test]
    fn preserves_url_without_credentials() {
        let url = "https://example.com/esplora/api";

        assert_eq!(url_for_logs(url), url);
    }

    #[test]
    fn malformed_url_is_not_logged() {
        let url = "not a URL with user:secret@example.com";

        let logged_url = url_for_logs(url);

        assert_eq!(logged_url, INVALID_URL_FOR_LOGS);
        assert!(!logged_url.contains("secret"));
    }
}
