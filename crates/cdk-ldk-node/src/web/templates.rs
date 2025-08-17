use maud::{html, Markup, DOCTYPE};

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

pub fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                link rel="icon" type="image/svg+xml" href="/static/favicon.svg";
                link rel="stylesheet" type="text/css" href="/static/css/globe.css";
                title { (title) " - CDK LDK Node" }
                style {
                    "
                    :root {
                        --background: 0 0% 100%;
                        --foreground: 222.2 84% 4.9%;
                        --card: 0 0% 100%;
                        --card-foreground: 222.2 84% 4.9%;
                        --popover: 0 0% 100%;
                        --popover-foreground: 222.2 84% 4.9%;
                        --primary: 222.2 47.4% 11.2%;
                        --primary-foreground: 210 40% 98%;
                        --secondary: 210 40% 96%;
                        --secondary-foreground: 222.2 84% 4.9%;
                        --muted: 210 40% 96%;
                        --muted-foreground: 215.4 16.3% 46.9%;
                        --accent: 210 40% 96%;
                        --accent-foreground: 222.2 84% 4.9%;
                        --destructive: 0 84.2% 60.2%;
                        --destructive-foreground: 210 40% 98%;
                        --border: 214.3 31.8% 91.4%;
                        --input: 214.3 31.8% 91.4%;
                        --ring: 222.2 84% 4.9%;
                        --radius: 0.5rem;
                        
                        /* Typography scale */
                        --fs-title: 1.25rem;
                        --fs-label: 0.8125rem;
                        --fs-value: 1.625rem;
                        
                        /* Line heights */
                        --lh-tight: 1.15;
                        --lh-normal: 1.4;
                        
                        /* Font weights */
                        --fw-medium: 500;
                        --fw-semibold: 600;
                        --fw-bold: 700;
                        
                        /* Colors */
                        --fg-primary: #0f172a;
                        --fg-muted: #6b7280;
                    }
                    
                    * {
                        box-sizing: border-box;
                        margin: 0;
                        padding: 0;
                    }
                    
                    html {
                        font-feature-settings: 'cv02', 'cv03', 'cv04', 'cv11';
                        font-variation-settings: normal;
                    }
                    
                    body {
                        font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Roboto', 'Oxygen', 'Ubuntu', 'Cantarell', 'Fira Sans', 'Droid Sans', 'Helvetica Neue', sans-serif;
                        font-size: 14px;
                        line-height: 1.5;
                        color: hsl(var(--foreground));
                        background-color: hsl(var(--background));
                        font-feature-settings: 'rlig' 1, 'calt' 1;
                        -webkit-font-smoothing: antialiased;
                        -moz-osx-font-smoothing: grayscale;
                        text-rendering: geometricPrecision;
                        min-height: 100vh;
                    }
                    
                    .container {
                        max-width: 1200px;
                        margin: 0 auto;
                        padding: 0 1rem;
                    }
                    
                    @media (min-width: 640px) {
                        .container {
                            padding: 0 2rem;
                        }
                    }
                    
                    /* Hero section styling */
                    header {
                        position: relative;
                        background-image: url('/static/images/bg.jpg?v=3');
                        background-size: cover;
                        background-position: center;
                        background-repeat: no-repeat;
                        border-bottom: 1px solid hsl(var(--border));
                        margin-bottom: 3rem;
                        text-align: center;
                        width: 100%;
                        height: 400px; /* Fixed height for better proportion */
                        display: flex;
                        align-items: center;
                        justify-content: center;
                    }
                    
                    /* Ensure text is positioned properly */
                    header .container {
                        position: absolute;
                        top: 50%;
                        left: 50%;
                        transform: translate(-50%, -50%);
                        z-index: 2;
                        width: 100%;
                        max-width: 1200px;
                        padding: 0 2rem;
                    }
                    
                    h1 {
                        font-size: 3rem;
                        font-weight: 700;
                        line-height: 1.1;
                        letter-spacing: -0.02em;
                        color: #000000;
                        margin-bottom: 1rem;
                    }
                    
                    .subtitle {
                        font-size: 1.25rem;
                        color: #333333;
                        font-weight: 400;
                        max-width: 600px;
                        margin: 0 auto;
                        line-height: 1.6;
                    }
                    
                    @media (max-width: 768px) {
                        header {
                            height: 300px; /* Smaller height on mobile */
                        }
                        
                        header .container {
                            padding: 0 1rem;
                        }
                        
                        h1 {
                            font-size: 2.25rem;
                        }
                        
                        .subtitle {
                            font-size: 1.1rem;
                        }
                    }
                    
                    /* Card fade-in animation */
                    @keyframes fade-in {
                        from { opacity: 0; transform: translateY(10px); }
                        to { opacity: 1; transform: translateY(0); }
                    }
                    
                    .card {
                        animation: fade-in 0.3s ease-out;
                    }
                    
                    /* Modern Navigation Bar Styling */
                    nav {
                        background-color: hsl(var(--card));
                        border-top: 1px solid hsl(var(--border));
                        border-bottom: 1px solid hsl(var(--border));
                        border-left: none;
                        border-right: none;
                        border-radius: 0;
                        padding: 0.75rem;
                        margin-bottom: 2rem;
                    }
                    
                    nav .container {
                        padding: 0;
                        display: flex;
                        justify-content: center;
                    }
                    
                    nav ul {
                        list-style: none;
                        display: flex;
                        gap: 0.5rem;
                        overflow-x: auto;
                        -webkit-overflow-scrolling: touch;
                        margin: 0;
                        padding: 0;
                        justify-content: center;
                    }
                    
                    nav li {
                        flex-shrink: 0;
                    }
                    
                    nav a {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        text-decoration: none;
                        font-size: 1rem;
                        font-weight: 600;
                        color: hsl(var(--muted-foreground));
                        padding: 1rem 1.5rem;
                        border-radius: calc(var(--radius) - 2px);
                        transition: all 200ms cubic-bezier(0.4, 0, 0.2, 1);
                        position: relative;
                        min-height: 3rem;
                    }
                    
                    nav a:hover {
                        color: hsl(var(--foreground));
                        background-color: hsl(var(--muted));
                    }
                    
                    nav a.active {
                        color: hsl(var(--primary-foreground));
                        background-color: hsl(var(--primary));
                        font-weight: 700;
                    }
                    
                    nav a.active:hover {
                        background-color: hsl(var(--primary) / 0.9);
                    }
                    
                    .card {
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                        box-shadow: 0 1px 2px 0 rgba(0, 0, 0, 0.05);
                    }
                    
                    /* Metric cards styling - matching balance-item style */
                    .metrics-container {
                        display: flex;
                        gap: 1rem;
                        margin: 1rem 0;
                        flex-wrap: wrap;
                    }
                    
                    .metric-card {
                        flex: 1;
                        min-width: 200px;
                        text-align: center;
                        padding: 1rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border-radius: calc(var(--radius) - 2px);
                        border: 1px solid hsl(var(--border));
                    }
                    
                    .metric-value {
                        font-size: 1.5rem;
                        font-weight: 600;
                        color: hsl(var(--foreground));
                        margin-bottom: 0.5rem;
                        line-height: 1.2;
                    }
                    
                    .metric-label {
                        font-size: 0.875rem;
                        color: hsl(var(--muted-foreground));
                        font-weight: 400;
                    }
                    
                    .card h2,
                    .section-title,
                    h2 {
                        font-size: var(--fs-title);
                        line-height: var(--lh-tight);
                        font-weight: var(--fw-semibold);
                        color: var(--fg-primary);
                        text-transform: none;
                        margin: 0 0 12px;
                    }
                    
                    h3 {
                        font-size: var(--fs-title);
                        line-height: var(--lh-tight);
                        font-weight: var(--fw-semibold);
                        color: var(--fg-primary);
                        text-transform: none;
                        margin: 0 0 12px;
                    }
                    
                    .form-group {
                        margin-bottom: 1.5rem;
                    }
                    
                    label {
                        display: block;
                        font-size: 0.875rem;
                        font-weight: 500;
                        color: hsl(var(--foreground));
                        margin-bottom: 0.5rem;
                    }
                    
                    input, textarea, select {
                        flex: 1;
                        background-color: hsl(var(--background));
                        border: 1px solid hsl(var(--input));
                        border-radius: calc(var(--radius) - 2px);
                        padding: 0.5rem 0.75rem;
                        font-size: 0.875rem;
                        line-height: 1.25;
                        color: hsl(var(--foreground));
                        transition: border-color 150ms ease-in-out, box-shadow 150ms ease-in-out;
                        width: 100%;
                    }
                    
                    input:focus, textarea:focus, select:focus {
                        outline: 2px solid transparent;
                        outline-offset: 2px;
                        border-color: hsl(var(--ring));
                        box-shadow: 0 0 0 2px hsl(var(--ring));
                    }
                    
                    input:disabled, textarea:disabled, select:disabled {
                        cursor: not-allowed;
                        opacity: 0.5;
                    }
                    
                    button {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        border-radius: calc(var(--radius) - 2px);
                        font-size: 0.875rem;
                        font-weight: 600;
                        transition: all 150ms ease-in-out;
                        border: 1px solid transparent;
                        cursor: pointer;
                        padding: 0.5rem 1rem;
                        height: 2.25rem;
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                    }
                    
                    button:hover {
                        background-color: hsl(var(--primary) / 0.9);
                    }
                    
                    button:focus-visible {
                        outline: 2px solid hsl(var(--ring));
                        outline-offset: 2px;
                    }
                    
                    button:disabled {
                        pointer-events: none;
                        opacity: 0.5;
                    }
                    
                    .button-secondary {
                        background-color: hsl(var(--secondary));
                        color: hsl(var(--secondary-foreground));
                        border: 1px solid hsl(var(--input));
                    }
                    
                    .button-secondary:hover {
                        background-color: hsl(var(--secondary) / 0.8);
                    }
                    
                    .button-outline {
                        border: 1px solid hsl(var(--input));
                        background-color: hsl(var(--background));
                        color: hsl(var(--foreground));
                    }
                    
                    .button-outline:hover {
                        background-color: hsl(var(--accent));
                        color: hsl(var(--accent-foreground));
                    }
                    
                    .button-destructive {
                        background-color: hsl(var(--destructive));
                        color: hsl(var(--destructive-foreground));
                    }
                    
                    .button-destructive:hover {
                        background-color: hsl(var(--destructive) / 0.9);
                    }
                    
                    .button-sm {
                        height: 2rem;
                        border-radius: calc(var(--radius) - 4px);
                        padding: 0 0.75rem;
                        font-size: 0.75rem;
                    }
                    
                    .button-lg {
                        height: 2.75rem;
                        border-radius: var(--radius);
                        padding: 0 2rem;
                        font-size: 1rem;
                    }
                    
                    .grid {
                        display: grid;
                        grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
                        gap: 1.5rem;
                    }
                    
                    @media (max-width: 640px) {
                        .grid {
                            grid-template-columns: 1fr;
                        }
                    }
                    

                    
                    .info-label,
                    .sub-label,
                    label {
                        font-size: var(--fs-label);
                        line-height: var(--lh-normal);
                        font-weight: var(--fw-medium);
                        color: var(--fg-muted);
                        text-transform: none;
                        letter-spacing: 0.02em;
                        flex-shrink: 0;
                    }
                    
                    .info-value {
                        font-size: 0.875rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: var(--fg-primary);
                        text-align: right;
                        word-break: break-all;
                        overflow-wrap: break-word;
                        hyphens: auto;
                        min-width: 0;
                    }
                    
                    .info-item {
                        display: flex;
                        gap: 0.5rem;
                        align-items: baseline;
                        margin: 8px 0;
                        padding: 1rem 0;
                        border-bottom: 1px solid hsl(var(--border));
                        min-height: 3rem;
                        justify-content: space-between;
                    }
                    
                    .info-item:last-child {
                        border-bottom: none;
                    }
                    
                    /* Card flex spacing improvements */
                    .card-flex {
                        display: flex;
                        gap: 1rem;
                        align-items: center;
                    }
                    
                    .card-flex-content {
                        flex: 1 1 auto;
                    }
                    
                    .card-flex-button {
                        flex: 0 0 auto;
                    }
                    
                    .card-flex-content p {
                        margin: 0 0 12px;
                        line-height: var(--lh-normal);
                    }
                    
                    .card-flex-content p + .card-flex-button,
                    .card-flex-content p + a,
                    .card-flex-content p + button {
                        margin-top: 12px;
                    }
                    
                    .card-flex-content .body + .card-flex-button,
                    .card-flex-content .body + a,
                    .card-flex-content .body + button {
                        margin-top: 12px;
                    }
                    
                    .truncate-value {
                        font-size: 0.875rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        text-align: right;
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                        display: inline-block;
                        max-width: 200px;
                    }
                    
                    .copy-button {
                        background-color: hsl(var(--secondary));
                        color: hsl(var(--secondary-foreground));
                        border: 1px solid hsl(var(--border));
                        border-radius: calc(var(--radius) - 4px);
                        padding: 0.25rem 0.5rem;
                        cursor: pointer;
                        font-size: 0.75rem;
                        font-weight: 600;
                        margin-left: 0.5rem;
                        transition: all 150ms ease-in-out;
                        height: auto;
                        min-height: auto;
                        flex-shrink: 0;
                    }
                    
                    .copy-button:hover {
                        background-color: hsl(var(--secondary) / 0.8);
                        border-color: hsl(var(--border));
                    }
                    
                    .balance-item,
                    .balance-item-container {
                        padding: 1.25rem 0;
                        border-bottom: 1px solid hsl(var(--border));
                        margin-bottom: 10px;
                    }
                    
                    .balance-item:last-child,
                    .balance-item-container:last-child {
                        border-bottom: none;
                    }
                    
                    .balance-item .balance-label,
                    .balance-item-container .balance-label,
                    .balance-title,
                    .balance-label {
                        display: block;
                        margin-bottom: 6px;
                        font-size: var(--fs-label);
                        line-height: var(--lh-normal);
                        font-weight: var(--fw-medium);
                        color: var(--fg-muted);
                        letter-spacing: 0.02em;
                        text-transform: none;
                    }
                    
                    .balance-item .balance-amount,
                    .balance-item-container .balance-value,
                    .balance-amount,
                    .balance-amount-value,
                    .balance-value {
                        display: block;
                        font-size: var(--fs-value);
                        line-height: var(--lh-tight);
                        font-weight: var(--fw-bold);
                        color: var(--fg-primary);
                        white-space: nowrap;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                    }
                    
                    .balance-item .info-label + .info-value,
                    .balance-item .label + .amount,
                    .balance-item-container .info-label + .info-value,
                    .balance-item-container .label + .amount {
                        margin-top: 6px;
                    }
                    
                    .alert {
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1rem;
                        margin-bottom: 1rem;
                    }
                    
                    .alert-success {
                        border-color: hsl(142.1 76.2% 36.3%);
                        background-color: hsl(142.1 70.6% 45.3% / 0.1);
                        color: hsl(142.1 76.2% 36.3%);
                    }
                    
                    .alert-destructive {
                        border-color: hsl(var(--destructive));
                        background-color: hsl(var(--destructive) / 0.1);
                        color: hsl(var(--destructive));
                    }
                    
                    .alert-warning {
                        border-color: hsl(32.6 75.4% 55.1%);
                        background-color: hsl(32.6 75.4% 55.1% / 0.1);
                        color: hsl(32.6 75.4% 55.1%);
                    }
                    
                    /* Legacy classes for backward compatibility */
                    .success {
                        border-color: hsl(142.1 76.2% 36.3%);
                        background-color: hsl(142.1 70.6% 45.3% / 0.1);
                        color: hsl(142.1 76.2% 36.3%);
                        border: 1px solid hsl(142.1 76.2% 36.3%);
                        border-radius: var(--radius);
                        padding: 1rem;
                        margin-bottom: 1rem;
                    }
                    
                    .error {
                        border-color: hsl(var(--destructive));
                        background-color: hsl(var(--destructive) / 0.1);
                        color: hsl(var(--destructive));
                        border: 1px solid hsl(var(--destructive));
                        border-radius: var(--radius);
                        padding: 1rem;
                        margin-bottom: 1rem;
                    }
                    
                    .badge {
                        display: inline-flex;
                        align-items: center;
                        border-radius: 9999px;
                        padding: 0.25rem 0.625rem;
                        font-size: 0.75rem;
                        font-weight: 500;
                        line-height: 1;
                        transition: all 150ms ease-in-out;
                        border: 1px solid transparent;
                    }
                    
                    .badge-default {
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                    }
                    
                    .badge-secondary {
                        background-color: hsl(var(--secondary));
                        color: hsl(var(--secondary-foreground));
                    }
                    
                    .badge-success {
                        background-color: hsl(142.1 70.6% 45.3%);
                        color: hsl(355.7 78% 98.4%);
                    }
                    
                    .badge-destructive {
                        background-color: hsl(var(--destructive));
                        color: hsl(var(--destructive-foreground));
                    }
                    
                    .badge-outline {
                        background-color: transparent;
                        color: hsl(var(--foreground));
                        border: 1px solid hsl(var(--border));
                    }
                    
                    /* Legacy status classes */
                    .status-badge {
                        display: inline-flex;
                        align-items: center;
                        border-radius: 9999px;
                        padding: 0.25rem 0.625rem;
                        font-size: 0.75rem;
                        font-weight: 500;
                        line-height: 1;
                    }
                    
                    .status-active {
                        background-color: hsl(142.1 70.6% 45.3%);
                        color: hsl(355.7 78% 98.4%);
                    }
                    
                    .status-inactive {
                        background-color: hsl(var(--destructive));
                        color: hsl(var(--destructive-foreground));
                    }
                    
                    .channel-item {
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                    }
                    
                    .channel-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 1rem;
                        gap: 1rem;
                    }
                    
                    .channel-id {
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 0.875rem;
                        color: hsl(var(--muted-foreground));
                        word-break: break-all;
                        flex: 1;
                        min-width: 0;
                    }
                    
                    .balance-info {
                        display: grid;
                        grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
                        gap: 1rem;
                        margin-top: 1rem;
                    }
                    
                    @media (max-width: 640px) {
                        .balance-info {
                            grid-template-columns: 1fr;
                        }
                    }
                    
                    .balance-item {
                        text-align: center;
                        padding: 1rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border-radius: calc(var(--radius) - 2px);
                        border: 1px solid hsl(var(--border));
                    }
                    
                    .balance-amount {
                        font-weight: 600;
                        font-size: 1.125rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        line-height: 1.2;
                    }
                    

                    
                    .payment-item {
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                    }
                    
                    .payment-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: flex-start;
                        margin-bottom: 1rem;
                        gap: 1rem;
                    }
                    
                    @media (max-width: 640px) {
                        .payment-header {
                            flex-direction: column;
                            align-items: stretch;
                            gap: 0.75rem;
                        }
                    }
                    
                    .payment-direction {
                        display: flex;
                        align-items: center;
                        gap: 0.5rem;
                        font-weight: 600;
                        color: hsl(var(--foreground));
                        flex: 1;
                        min-width: 0;
                    }
                    
                    .direction-icon {
                        font-size: 1.125rem;
                        font-weight: bold;
                        color: hsl(var(--muted-foreground));
                    }
                    
                    .payment-details {
                        display: flex;
                        flex-direction: column;
                        gap: 0.75rem;
                    }
                    
                    .payment-amount {
                        font-size: 1.25rem;
                        font-weight: 600;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        line-height: 1.2;
                    }
                    
                    .payment-info {
                        display: flex;
                        align-items: center;
                        gap: 0.75rem;
                        flex-wrap: wrap;
                    }
                    
                    @media (max-width: 640px) {
                        .payment-info {
                            flex-direction: column;
                            align-items: flex-start;
                            gap: 0.25rem;
                        }
                    }
                    
                    .payment-label {
                        font-weight: 500;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.875rem;
                        flex-shrink: 0;
                    }
                    
                    .payment-value {
                        color: hsl(var(--foreground));
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 0.875rem;
                        word-break: break-all;
                        min-width: 0;
                    }
                    
                    .payment-list-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 1.5rem;
                        padding-bottom: 1rem;
                        border-bottom: 1px solid hsl(var(--border));
                    }
                    
                    @media (max-width: 640px) {
                        .payment-list-header {
                            flex-direction: column;
                            align-items: stretch;
                            gap: 1rem;
                        }
                    }
                    
                    .payment-filter-tabs {
                        display: flex;
                        gap: 0.25rem;
                        overflow-x: auto;
                        -webkit-overflow-scrolling: touch;
                    }
                    
                    .payment-filter-tab {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        padding: 0.5rem 1rem;
                        border: 1px solid hsl(var(--border));
                        background-color: hsl(var(--background));
                        border-radius: calc(var(--radius) - 2px);
                        text-decoration: none;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.875rem;
                        font-weight: 600;
                        transition: all 150ms ease-in-out;
                        height: 2.25rem;
                    }
                    
                    .payment-filter-tab:hover {
                        background-color: hsl(var(--accent));
                        color: hsl(var(--accent-foreground));
                        text-decoration: none;
                    }
                    
                    .payment-filter-tab.active {
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                        border-color: hsl(var(--primary));
                    }
                    
                    .payment-type-badge {
                        display: inline-flex;
                        align-items: center;
                        border-radius: 9999px;
                        padding: 0.125rem 0.5rem;
                        font-size: 0.625rem;
                        font-weight: 600;
                        line-height: 1;
                        margin-left: 0.5rem;
                        text-transform: uppercase;
                        letter-spacing: 0.05em;
                    }
                    
                    .payment-type-bolt11 {
                        background-color: hsl(217 91% 60% / 0.1);
                        color: hsl(217 91% 60%);
                        border: 1px solid hsl(217 91% 60% / 0.2);
                    }
                    
                    .payment-type-bolt12 {
                        background-color: hsl(262 83% 58% / 0.1);
                        color: hsl(262 83% 58%);
                        border: 1px solid hsl(262 83% 58% / 0.2);
                    }
                    
                    .payment-type-onchain {
                        background-color: hsl(32 95% 44% / 0.1);
                        color: hsl(32 95% 44%);
                        border: 1px solid hsl(32 95% 44% / 0.2);
                    }
                    
                    .payment-type-spontaneous {
                        background-color: hsl(142.1 70.6% 45.3% / 0.1);
                        color: hsl(142.1 70.6% 45.3%);
                        border: 1px solid hsl(142.1 70.6% 45.3% / 0.2);
                    }
                    
                    .payment-type-bolt11-jit {
                        background-color: hsl(199 89% 48% / 0.1);
                        color: hsl(199 89% 48%);
                        border: 1px solid hsl(199 89% 48% / 0.2);
                    }
                    
                    .payment-type-unknown {
                        background-color: hsl(var(--muted));
                        color: hsl(var(--muted-foreground));
                        border: 1px solid hsl(var(--border));
                    }
                    
                    /* Pagination */
                    .pagination-controls {
                        display: flex;
                        justify-content: center;
                        align-items: center;
                        margin: 2rem 0;
                    }
                    
                    .pagination {
                        display: flex;
                        align-items: center;
                        gap: 0.25rem;
                        list-style: none;
                    }
                    
                    .pagination-btn, .pagination-number {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        border-radius: calc(var(--radius) - 2px);
                        font-size: 0.875rem;
                        font-weight: 600;
                        transition: all 150ms ease-in-out;
                        border: 1px solid hsl(var(--border));
                        background-color: hsl(var(--background));
                        color: hsl(var(--foreground));
                        text-decoration: none;
                        cursor: pointer;
                        height: 2.25rem;
                        min-width: 2.25rem;
                        padding: 0 0.5rem;
                    }
                    
                    .pagination-btn:hover, .pagination-number:hover {
                        background-color: hsl(var(--accent));
                        color: hsl(var(--accent-foreground));
                        text-decoration: none;
                    }
                    
                    .pagination-number.active {
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                        border-color: hsl(var(--primary));
                    }
                    
                    .pagination-btn.disabled {
                        background-color: hsl(var(--muted));
                        color: hsl(var(--muted-foreground));
                        cursor: not-allowed;
                        opacity: 0.5;
                        pointer-events: none;
                    }
                    
                    .pagination-ellipsis {
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        height: 2.25rem;
                        width: 2.25rem;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.875rem;
                    }
                    
                    /* Responsive adjustments */
                    @media (max-width: 640px) {
                        .container {
                            padding: 0 1rem;
                        }
                        
                        header {
                            padding: 1rem 0;
                            margin-bottom: 1rem;
                        }
                        
                        h1 {
                            font-size: 1.5rem;
                        }
                        
                        nav ul {
                            flex-wrap: wrap;
                        }
                        
                        .card {
                            padding: 1rem;
                            margin-bottom: 1rem;
                        }
                        
                        .info-item {
                            flex-direction: column;
                            align-items: flex-start;
                            gap: 0.75rem;
                            padding: 1rem 0;
                            min-height: auto;
                        }
                        
                        .info-value, .truncate-value {
                            text-align: left;
                            max-width: 100%;
                        }
                        
                        .copy-button {
                            margin-left: 0;
                            margin-top: 0.25rem;
                            align-self: flex-start;
                        }
                        
                        .balance-amount-value {
                            font-size: 1.25rem;
                        }
                        
                        .pagination {
                            flex-wrap: wrap;
                            justify-content: center;
                            gap: 0.125rem;
                        }
                        
                        .pagination-btn, .pagination-number {
                            height: 2rem;
                            min-width: 2rem;
                            font-size: 0.75rem;
                        }
                    }
                    
                    /* Node Information Section Styling */
                    .node-info-section {
                        display: flex;
                        gap: 1.5rem;
                        margin-bottom: 1.5rem;
                        align-items: flex-start;
                    }
                    
                    .node-info-main-container {
                        flex: 1;
                        display: flex;
                        flex-direction: column;
                        gap: 1rem;
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        box-shadow: 0 1px 2px 0 rgba(0, 0, 0, 0.05);
                    }
                    
                    .node-info-left {
                        display: flex;
                        align-items: center;
                        gap: 1rem;
                        margin-bottom: 1rem;
                    }
                    
                    .node-avatar {
                        flex-shrink: 0;
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 0.75rem;
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        width: 80px;
                        height: 80px;
                    }
                    
                    .avatar-image {
                        width: 48px;
                        height: 48px;
                        border-radius: calc(var(--radius) - 2px);
                        object-fit: cover;
                        display: block;
                    }
                    
                    .node-details {
                        flex: 1;
                        min-width: 0;
                    }
                    
                    .node-name {
                        font-size: var(--fs-title);
                        font-weight: var(--fw-semibold);
                        color: var(--fg-primary);
                        margin: 0 0 0.25rem 0;
                        line-height: var(--lh-tight);
                        word-wrap: break-word;
                        overflow-wrap: break-word;
                        hyphens: auto;
                    }
                    
                    .node-address {
                        font-size: 0.875rem;
                        color: var(--fg-muted);
                        margin: 0;
                        line-height: var(--lh-normal);
                    }
                    
                    .node-content-box {
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        min-height: 200px;
                        padding: 1rem;
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        color: hsl(var(--muted-foreground));
                        overflow: hidden;
                    }
                    
                    .node-metrics {
                        flex-shrink: 0;
                        width: 280px;
                        display: flex;
                        flex-direction: column;
                    }
                    
                    .node-metrics .card {
                        margin-bottom: 0;
                        flex: 1;
                        display: flex;
                        flex-direction: column;
                    }
                    
                    .node-metrics .metrics-container {
                        flex-direction: column;
                        margin: 1rem 0 0 0;
                        flex: 1;
                    }
                    
                    .node-metrics .metric-card {
                        min-width: auto;
                    }
                    
                    /* Mobile responsive design for node info */
                    @media (max-width: 768px) {
                        .node-info-section {
                            flex-direction: column;
                            gap: 1rem;
                        }
                        
                        .node-info-left {
                            flex-direction: column;
                            align-items: flex-start;
                            text-align: center;
                            gap: 0.75rem;
                        }
                        
                        .node-avatar {
                            align-self: center;
                        }
                        
                        .node-details {
                            text-align: center;
                            width: 100%;
                        }
                        
                        .node-content-box {
                            min-height: 150px;
                            padding: 1rem;
                        }
                        
                        .node-metrics {
                            width: 100%;
                        }
                        
                        .node-metrics .metrics-container {
                            flex-direction: row;
                            flex-wrap: wrap;
                        }
                        
                        .node-metrics .metric-card {
                            flex: 1;
                            min-width: 120px;
                        }
                    }
                    
                    @media (max-width: 480px) {
                        .node-info-left {
                            gap: 0.5rem;
                        }
                        
                        .node-avatar {
                            width: 64px;
                            height: 64px;
                            padding: 0.5rem;
                        }
                        
                        .avatar-image {
                            width: 40px;
                            height: 40px;
                        }
                        
                        .node-name {
                            font-size: 1rem;
                            word-wrap: break-word;
                            overflow-wrap: break-word;
                            hyphens: auto;
                        }
                        
                        .node-address {
                            font-size: 0.8125rem;
                        }
                        
                        .node-content-box {
                            min-height: 120px;
                            padding: 0.75rem;
                        }
                        
                        .node-metrics .metrics-container {
                            flex-direction: column;
                            gap: 0.75rem;
                        }
                    }

                    /* Responsive typography adjustments */
                    @media (max-width: 640px) {
                        :root {
                            --fs-value: 1.45rem;
                        }
                        
                        .node-name {
                            font-size: 0.875rem;
                        }
                    }
                    
                    @media (max-width: 480px) {
                        .node-name {
                            font-size: 0.8125rem;
                        }
                    }
                    "
                }
            }
            body {
                header {
                    div class="container" {
                        h1 { "CDK LDK Node" }
                        p class="subtitle" { "Lightning Network Node Management" }
                    }
                }

                nav {
                    div class="container" {
                        ul {
                            li { a href="/" { "Dashboard" } }
                            li { a href="/balance" { "Lightning" } }
                            li { a href="/onchain" { "On-chain" } }
                            li { a href="/invoices" { "Invoices" } }
                            li { a href="/payments" { "All Payments" } }
                        }
                    }
                }

                main class="container" {
                    (content)
                }
            }
        }
    }
}

pub fn info_card(title: &str, items: Vec<(&str, String)>) -> Markup {
    html! {
        div class="card" {
            h2 { (title) }
            @for (label, value) in items {
                div class="info-item" {
                    span class="info-label" { (label) ":" }
                    span class="info-value" { (value) }
                }
            }
        }
    }
}

pub fn form_card(title: &str, form_content: Markup) -> Markup {
    html! {
        div class="card" {
            h2 { (title) }
            (form_content)
        }
    }
}

pub fn success_message(message: &str) -> Markup {
    html! {
        div class="success" { (message) }
    }
}

pub fn error_message(message: &str) -> Markup {
    html! {
        div class="error" { (message) }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn payment_list_item(
    _payment_id: &str,
    direction: &str,
    status: &str,
    amount: &str,
    payment_hash: Option<&str>,
    description: Option<&str>,
    timestamp: Option<u64>,
    payment_type: &str,
    preimage: Option<&str>,
) -> Markup {
    let status_class = match status {
        "Succeeded" => "status-active",
        "Failed" => "status-inactive",
        "Pending" => "status-badge",
        _ => "status-badge",
    };

    let direction_icon = match direction {
        "Inbound" => "↓",
        "Outbound" => "↑",
        _ => "•",
    };

    let type_class = match payment_type {
        "BOLT11" => "payment-type-bolt11",
        "BOLT12" => "payment-type-bolt12",
        "On-chain" => "payment-type-onchain",
        "Spontaneous" => "payment-type-spontaneous",
        "BOLT11 JIT" => "payment-type-bolt11-jit",
        _ => "payment-type-unknown",
    };

    html! {
        div class="payment-item" {
            div class="payment-header" {
                div class="payment-direction" {
                    span class="direction-icon" { (direction_icon) }
                    span { (direction) " Payment" }
                    span class=(format!("payment-type-badge {}", type_class)) { (payment_type) }
                }
                span class=(format!("status-badge {}", status_class)) { (status) }
            }

            div class="payment-details" {
                div class="payment-amount" { (amount) }

                @if let Some(hash) = payment_hash {
                    div class="payment-info" {
                        span class="payment-label" {
                            @if payment_type == "BOLT11" || payment_type == "BOLT12" || payment_type == "Spontaneous" || payment_type == "BOLT11 JIT" { "Payment Hash:" }
                            @else { "Transaction ID:" }
                        }
                        span class="payment-value" title=(hash) {
                            (&hash[..std::cmp::min(16, hash.len())]) "..."
                        }
                        button class="copy-button" data-copy=(hash)
                               onclick="navigator.clipboard.writeText(this.getAttribute('data-copy')).then(() => { this.textContent = 'Copied!'; setTimeout(() => this.textContent = 'Copy', 2000); })" {
                            "Copy"
                        }
                    }
                }

                // Show preimage for successful outgoing BOLT11 or BOLT12 payments
                @if let Some(preimage_str) = preimage {
                    @if !preimage_str.is_empty() && direction == "Outbound" && status == "Succeeded" && (payment_type == "BOLT11" || payment_type == "BOLT12") {
                        div class="payment-info" {
                            span class="payment-label" { "Preimage:" }
                            span class="payment-value" title=(preimage_str) {
                                (&preimage_str[..std::cmp::min(16, preimage_str.len())]) "..."
                            }
                            button class="copy-button" data-copy=(preimage_str)
                                   onclick="navigator.clipboard.writeText(this.getAttribute('data-copy')).then(() => { this.textContent = 'Copied!'; setTimeout(() => this.textContent = 'Copy', 2000); })" {
                                "Copy"
                            }
                        }
                    }
                }

                @if let Some(desc) = description {
                    @if !desc.is_empty() {
                        div class="payment-info" {
                            span class="payment-label" { "Description:" }
                            span class="payment-value" { (desc) }
                        }
                    }
                }

                @if let Some(ts) = timestamp {
                    div class="payment-info" {
                        span class="payment-label" { "Last Update:" }
                        span class="payment-value" {
                            (format_timestamp(ts))
                        }
                    }
                }
            }
        }
    }
}
