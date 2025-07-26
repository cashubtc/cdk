use maud::{html, Markup, DOCTYPE};

/// Format satoshis as a whole number with Bitcoin symbol (BIP177)
pub fn format_sats_as_btc(sats: u64) -> String {
    format!("₿{}", sats)
}

/// Format millisatoshis as satoshis (whole number) with Bitcoin symbol (BIP177)
pub fn format_msats_as_btc(msats: u64) -> String {
    let sats = msats / 1000;
    format!("₿{}", sats)
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
                            li { a href="/payments" { "Payments" } }
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
