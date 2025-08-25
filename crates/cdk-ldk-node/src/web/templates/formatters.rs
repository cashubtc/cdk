/// Format satoshis as a whole number with Bitcoin symbol (BIP177)
pub fn format_sats_as_btc(sats: u64) -> String {
    let sats_str = sats.to_string();
    let formatted_sats = if sats_str.len() > 3 {
        let mut result = String::new();
        let chars: Vec<char> = sats_str.chars().collect();
        let len = chars.len();

        for (i, ch) in chars.iter().enumerate() {
            // Add comma before every group of 3 digits from right to left
            if i > 0 && (len - i) % 3 == 0 {
                result.push(',');
            }
            result.push(*ch);
        }
        result
    } else {
        sats_str
    };

    format!("₿{formatted_sats}")
}

/// Format millisatoshis as satoshis (whole number) with Bitcoin symbol (BIP177)
pub fn format_msats_as_btc(msats: u64) -> String {
    let sats = msats / 1000;
    let sats_str = sats.to_string();
    let formatted_sats = if sats_str.len() > 3 {
        let mut result = String::new();
        let chars: Vec<char> = sats_str.chars().collect();
        let len = chars.len();

        for (i, ch) in chars.iter().enumerate() {
            // Add comma before every group of 3 digits from right to left
            if i > 0 && (len - i) % 3 == 0 {
                result.push(',');
            }
            result.push(*ch);
        }
        result
    } else {
        sats_str
    };

    format!("₿{formatted_sats}")
}

/// Format a Unix timestamp as a human-readable date and time
pub fn format_timestamp(timestamp: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(timestamp);

    match diff {
        0..=60 => "Just now".to_string(),
        61..=3600 => format!("{} min ago", diff / 60),
        _ => {
            // For timestamps older than 1 hour, show UTC time
            // Convert to a simple UTC format
            let total_seconds = timestamp;
            let seconds = total_seconds % 60;
            let total_minutes = total_seconds / 60;
            let minutes = total_minutes % 60;
            let total_hours = total_minutes / 60;
            let hours = total_hours % 24;
            let days = total_hours / 24;

            // Calculate year, month, day from days since epoch (1970-01-01)
            let mut year = 1970;
            let mut remaining_days = days;

            // Simple year calculation
            loop {
                let is_leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
                let days_in_year = if is_leap_year { 366 } else { 365 };

                if remaining_days >= days_in_year {
                    remaining_days -= days_in_year;
                    year += 1;
                } else {
                    break;
                }
            }

            // Calculate month and day
            let is_leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            let days_in_months = if is_leap_year {
                [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
            } else {
                [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
            };

            let mut month = 1;
            let mut day = remaining_days + 1;

            for &days_in_month in &days_in_months {
                if day > days_in_month {
                    day -= days_in_month;
                    month += 1;
                } else {
                    break;
                }
            }

            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                year, month, day, hours, minutes, seconds
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Test "Just now" (30 seconds ago)
        let recent = now - 30;
        assert_eq!(format_timestamp(recent), "Just now");

        // Test minutes ago (30 minutes ago)
        let minutes_ago = now - (30 * 60);
        assert_eq!(format_timestamp(minutes_ago), "30 min ago");

        // Test UTC format for older timestamps (2 hours ago)
        let hours_ago = now - (2 * 60 * 60);
        let result = format_timestamp(hours_ago);
        assert!(result.ends_with(" UTC"));
        assert!(result.contains("-"));
        assert!(result.contains(":"));

        // Test known timestamp: January 1, 2020 00:00:00 UTC
        let timestamp_2020 = 1577836800; // 2020-01-01 00:00:00 UTC
        let result = format_timestamp(timestamp_2020);
        assert_eq!(result, "2020-01-01 00:00:00 UTC");
    }
}
