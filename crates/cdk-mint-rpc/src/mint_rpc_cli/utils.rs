/// Parses a comma-separated string into a vector of trimmed strings
pub fn parse_csv(s: &Option<String>) -> Vec<String> {
    s.as_ref()
        .map(|v| v.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default()
}
