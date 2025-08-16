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
                title { (title) " - CDK LDK Node" }
                style {
                    "
                    * {
                        margin: 0;
                        padding: 0;
                        box-sizing: border-box;
                    }
                    
                    body {
                        font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
                        line-height: 1.6;
                        color: #333;
                        background-color: #f4f4f4;
                    }
                    
                    .container {
                        max-width: 1200px;
                        margin: 0 auto;
                        padding: 20px;
                    }
                    
                    header {
                        background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                        color: white;
                        padding: 1rem 0;
                        margin-bottom: 2rem;
                        box-shadow: 0 2px 10px rgba(0,0,0,0.1);
                    }
                    
                    h1 {
                        text-align: center;
                        font-size: 2.5em;
                        margin-bottom: 0.5rem;
                    }
                    
                    .subtitle {
                        text-align: center;
                        font-size: 1.1em;
                        opacity: 0.9;
                    }
                    
                    nav {
                        background: white;
                        padding: 1rem 0;
                        margin-bottom: 2rem;
                        box-shadow: 0 2px 5px rgba(0,0,0,0.1);
                    }
                    
                    nav ul {
                        list-style: none;
                        display: flex;
                        justify-content: center;
                        flex-wrap: wrap;
                        gap: 2rem;
                    }
                    
                    nav a {
                        text-decoration: none;
                        color: #667eea;
                        font-weight: 600;
                        padding: 0.5rem 1rem;
                        border-radius: 5px;
                        transition: all 0.3s ease;
                    }
                    
                    nav a:hover {
                        background: #667eea;
                        color: white;
                    }
                    
                    .card {
                        background: white;
                        border-radius: 10px;
                        padding: 2rem;
                        margin-bottom: 2rem;
                        box-shadow: 0 4px 15px rgba(0,0,0,0.1);
                        transition: transform 0.3s ease;
                    }
                    
                    .card-flex {
                        display: flex;
                        flex-direction: column;
                        height: 100%;
                    }
                    
                    .card-flex-content {
                        flex-grow: 1;
                    }
                    
                    .card-flex-button {
                        margin-top: auto;
                    }
                    
                    .card:hover {
                        transform: translateY(-5px);
                    }
                    
                    .form-group {
                        margin-bottom: 1rem;
                    }
                    
                    label {
                        display: block;
                        margin-bottom: 0.5rem;
                        font-weight: 600;
                        color: #555;
                    }
                    
                    input, textarea, select {
                        width: 100%;
                        padding: 0.75rem;
                        border: 2px solid #ddd;
                        border-radius: 5px;
                        font-size: 1rem;
                        transition: border-color 0.3s ease;
                    }
                    
                    input:focus, textarea:focus, select:focus {
                        outline: none;
                        border-color: #667eea;
                    }
                    
                    button {
                        background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                        color: white;
                        padding: 0.75rem 2rem;
                        border: none;
                        border-radius: 5px;
                        font-size: 1rem;
                        font-weight: 600;
                        cursor: pointer;
                        transition: all 0.3s ease;
                    }
                    
                    button:hover {
                        transform: translateY(-2px);
                        box-shadow: 0 5px 15px rgba(0,0,0,0.2);
                    }
                    
                    .grid {
                        display: grid;
                        grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
                        gap: 2rem;
                    }
                    
                    .info-item {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        padding: 0.5rem 0;
                        border-bottom: 1px solid #eee;
                    }
                    
                    .info-item:last-child {
                        border-bottom: none;
                    }
                    
                    .info-label {
                        font-weight: 600;
                        color: #555;
                    }
                    
                    .info-value {
                        color: #333;
                        word-break: break-all;
                        max-width: 60%;
                        text-align: right;
                    }
                    
                    .truncate-value {
                        color: #333;
                        max-width: 60%;
                        text-align: right;
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                        display: inline-block;
                    }
                    
                    .copy-button {
                        background: #f1f1f1;
                        color: #333;
                        border: none;
                        padding: 0.25rem 0.5rem;
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.8rem;
                        margin-left: 0.5rem;
                        transition: background 0.3s ease;
                    }
                    
                    .copy-button:hover {
                        background: #e1e1e1;
                    }
                    
                    /* Specific styles for balance items to prevent overflow */
                    .balance-item-container {
                        display: flex;
                        flex-direction: column;
                        gap: 0.25rem;
                        padding: 0.75rem 0;
                        border-bottom: 1px solid #eee;
                    }
                    
                    .balance-item-container:last-child {
                        border-bottom: none;
                    }
                    
                    .balance-title {
                        font-weight: 600;
                        color: #555;
                        font-size: 0.9rem;
                    }
                    
                    .balance-amount-value {
                        color: #333;
                        font-size: 1.1rem;
                        word-break: break-all;
                        text-align: right;
                    }
                    
                    .success {
                        background: #d4edda;
                        color: #155724;
                        padding: 1rem;
                        border: 1px solid #c3e6cb;
                        border-radius: 5px;
                        margin-bottom: 1rem;
                    }
                    
                    .error {
                        background: #f8d7da;
                        color: #721c24;
                        padding: 1rem;
                        border: 1px solid #f5c6cb;
                        border-radius: 5px;
                        margin-bottom: 1rem;
                    }
                    
                    .status-badge {
                        padding: 0.25rem 0.75rem;
                        border-radius: 20px;
                        font-size: 0.8rem;
                        font-weight: 600;
                    }
                    
                    .status-active {
                        background: #d4edda;
                        color: #155724;
                    }
                    
                    .status-inactive {
                        background: #f8d7da;
                        color: #721c24;
                    }
                    
                    .channel-item {
                        background: #f8f9fa;
                        border-radius: 8px;
                        padding: 1rem;
                        margin-bottom: 1rem;
                        border: 1px solid #e9ecef;
                    }
                    
                    .channel-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 0.5rem;
                    }
                    
                    .channel-id {
                        font-family: monospace;
                        font-size: 0.9rem;
                        color: #6c757d;
                        word-break: break-all;
                    }
                    
                    .balance-info {
                        display: grid;
                        grid-template-columns: 1fr 1fr 1fr;
                        gap: 1rem;
                        margin-top: 0.5rem;
                    }
                    
                    .balance-item {
                        text-align: center;
                        padding: 0.5rem;
                        background: white;
                        border-radius: 5px;
                        border: 1px solid #dee2e6;
                    }
                    
                    .balance-amount {
                        font-weight: 600;
                        font-size: 1.1em;
                        color: #495057;
                    }
                    
                    .balance-label {
                        font-size: 0.8rem;
                        color: #6c757d;
                        margin-top: 0.25rem;
                    }
                    
                    .payment-item {
                        background: #f8f9fa;
                        border-radius: 8px;
                        padding: 1rem;
                        margin-bottom: 1rem;
                        border: 1px solid #e9ecef;
                        transition: box-shadow 0.3s ease;
                    }
                    
                    .payment-item:hover {
                        box-shadow: 0 4px 12px rgba(0,0,0,0.1);
                    }
                    
                    .payment-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 0.75rem;
                    }
                    
                    .payment-direction {
                        display: flex;
                        align-items: center;
                        gap: 0.5rem;
                        font-weight: 600;
                        color: #495057;
                    }
                    
                    .direction-icon {
                        font-size: 1.2em;
                        font-weight: bold;
                    }
                    
                    .payment-details {
                        display: flex;
                        flex-direction: column;
                        gap: 0.5rem;
                    }
                    
                    .payment-amount {
                        font-size: 1.2em;
                        font-weight: 600;
                        color: #212529;
                    }
                    
                    .payment-info {
                        display: flex;
                        align-items: center;
                        gap: 0.5rem;
                    }
                    
                    .payment-label {
                        font-weight: 500;
                        color: #6c757d;
                        min-width: 100px;
                    }
                    
                    .payment-value {
                        color: #495057;
                        font-family: monospace;
                        font-size: 0.9rem;
                    }
                    
                    .payment-list-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 1rem;
                        padding-bottom: 0.5rem;
                        border-bottom: 2px solid #e9ecef;
                    }
                    
                    .payment-filter-tabs {
                        display: flex;
                        gap: 0.5rem;
                    }
                    
                    .payment-filter-tab {
                        padding: 0.5rem 1rem;
                        border: 1px solid #dee2e6;
                        background: #f8f9fa;
                        border-radius: 5px;
                        text-decoration: none;
                        color: #495057;
                        font-weight: 500;
                        transition: all 0.3s ease;
                    }
                    
                    .payment-filter-tab:hover {
                        background: #e9ecef;
                        text-decoration: none;
                        color: #495057;
                    }
                    
                    .payment-filter-tab.active {
                        background: #667eea;
                        color: white;
                        border-color: #667eea;
                    }
                    
                    .payment-type-badge {
                        padding: 0.25rem 0.5rem;
                        border-radius: 12px;
                        font-size: 0.7rem;
                        font-weight: 600;
                        margin-left: 0.5rem;
                        text-transform: uppercase;
                    }
                    
                    .payment-type-bolt11 {
                        background: #e3f2fd;
                        color: #1976d2;
                        border: 1px solid #bbdefb;
                    }
                    
                    .payment-type-bolt12 {
                        background: #f3e5f5;
                        color: #7b1fa2;
                        border: 1px solid #ce93d8;
                    }
                    
                    .payment-type-onchain {
                        background: #fff3e0;
                        color: #f57c00;
                        border: 1px solid #ffcc02;
                    }
                    
                    .payment-type-spontaneous {
                        background: #e8f5e8;
                        color: #2e7d32;
                        border: 1px solid #4caf50;
                    }
                    
                    .payment-type-bolt11-jit {
                        background: #e1f5fe;
                        color: #0277bd;
                        border: 1px solid #03a9f4;
                    }
                    
                    .payment-type-unknown {
                        background: #f5f5f5;
                        color: #757575;
                        border: 1px solid #e0e0e0;
                    }
                    
                    /* Pagination styles */
                    .pagination-controls {
                        display: flex;
                        justify-content: center;
                        align-items: center;
                        gap: 1rem;
                        margin: 1rem 0;
                    }
                    
                    .pagination {
                        display: flex;
                        align-items: center;
                        gap: 0.5rem;
                        list-style: none;
                    }
                    
                    .pagination-btn, .pagination-number {
                        padding: 0.5rem 0.75rem;
                        border: 1px solid #dee2e6;
                        background: white;
                        color: #495057;
                        text-decoration: none;
                        border-radius: 4px;
                        font-size: 0.9rem;
                        transition: all 0.3s ease;
                        cursor: pointer;
                        min-width: 40px;
                        text-align: center;
                    }
                    
                    .pagination-btn:hover, .pagination-number:hover {
                        background: #e9ecef;
                        text-decoration: none;
                        color: #495057;
                    }
                    
                    .pagination-number.active {
                        background: #667eea;
                        color: white;
                        border-color: #667eea;
                    }
                    
                    .pagination-btn.disabled {
                        background: #f8f9fa;
                        color: #6c757d;
                        cursor: not-allowed;
                        opacity: 0.6;
                    }
                    
                    .pagination-btn.disabled:hover {
                        background: #f8f9fa;
                        color: #6c757d;
                    }
                    
                    .pagination-ellipsis {
                        padding: 0.5rem 0.25rem;
                        color: #6c757d;
                        font-size: 0.9rem;
                    }
                    
                    @media (max-width: 768px) {
                        .container {
                            padding: 10px;
                        }
                        
                        nav ul {
                            flex-direction: column;
                            gap: 0.5rem;
                        }
                        
                        .grid {
                            grid-template-columns: 1fr;
                        }
                        
                        .balance-info {
                            grid-template-columns: 1fr;
                        }
                        
                        .info-item {
                            flex-direction: column;
                            align-items: flex-start;
                        }
                        
                        .info-value {
                            max-width: 100%;
                            text-align: left;
                        }
                        
                        .pagination-controls {
                            flex-direction: column;
                            gap: 0.75rem;
                        }
                        
                        .pagination {
                            flex-wrap: wrap;
                            justify-content: center;
                        }
                        
                        .pagination-btn, .pagination-number {
                            padding: 0.4rem 0.6rem;
                            font-size: 0.8rem;
                            min-width: 35px;
                        }
                    }
                    "
                }
            }
            body {
                header {
                    div class="container" {
                        h1 { "CDK LDK Node" }
                        div class="subtitle" { "Lightning Network Node Management" }
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

pub fn balance_card(title: &str, items: Vec<(&str, String)>) -> Markup {
    html! {
        div class="card" {
            h2 { (title) }
            @for (label, value) in items {
                div class="balance-item-container" {
                    span class="balance-title" { (label) ":" }
                    span class="balance-amount-value" { (value) }
                }
            }
        }
    }
}

pub fn usage_metrics_card(title: &str, items: Vec<(&str, String)>) -> Markup {
    html! {
        div class="card" {
            h3 { (title) }
            @for (label, value) in items {
                div class="balance-item-container" {
                    span class="balance-title" { (label) ":" }
                    span class="balance-amount-value" { (value) }
                }
            }
        }
    }
}

pub fn info_card_with_copy(title: &str, items: Vec<(&str, String)>) -> Markup {
    html! {
        div class="card" {
            h2 { (title) }
            @for (label, value) in items {
                div class="info-item" {
                    span class="info-label" { (label) ":" }
                    @if label == "Node ID" {
                        span class="truncate-value" title=(value) { (&value[..std::cmp::min(20, value.len())]) "..." }
                        button class="copy-button" data-copy=(value) onclick="navigator.clipboard.writeText(this.getAttribute('data-copy')).then(() => { this.textContent = 'Copied!'; setTimeout(() => this.textContent = 'Copy', 2000); })" { "Copy" }
                    } @else {
                        span class="info-value" { (value) }
                    }
                }
            }
        }
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
