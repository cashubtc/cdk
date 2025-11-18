use maud::{html, Markup};

pub fn info_card(title: &str, items: Vec<(&str, String)>) -> Markup {
    html! {
        div class="card" {
            h2 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 0;" { (title) }
            div style="margin-top: 1.5rem;" {
                @for (label, value) in items {
                    div class="info-item" {
                        span class="info-label" { (label) ":" }
                        span class="info-value" { (value) }
                    }
                }
            }
        }
    }
}

pub fn form_card(title: &str, form_content: Markup) -> Markup {
    html! {
        div class="card" {
            h2 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 0;" { (title) }
            div style="margin-top: 1.5rem;" {
                (form_content)
            }
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

pub fn invoice_display_card(
    invoice_text: &str,
    amount: &str,
    details: Vec<(&str, String)>,
    back_url: &str,
) -> Markup {
    html! {
        div class="card" {
            div style="display: flex; justify-content: space-between; align-items: center; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 1.5rem;" {
                h3 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; margin: 0;" { "Invoice Details" }
            }

            // Amount highlight section at the top
            div class="invoice-amount-section" {
                div class="invoice-amount-label" { "Amount" }
                div class="invoice-amount-value" { (amount) }
            }

            // Invoice display section - under the amount
            div class="invoice-display-section" {
                div class="invoice-label" { "Invoice" }
                div class="invoice-display-container" {
                    textarea readonly class="invoice-textarea" { (invoice_text) }
                }
            }

            // Invoice details section - after the invoice with increased spacing
            div class="invoice-details-section" style="margin-top: 2.5rem;" {
                h4 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; margin: 0 0 1rem 0;" { "Details" }
                @for (label, value) in details {
                    div class="info-item" {
                        span class="info-label" { (label) ":" }
                        span class="info-value" { (value) }
                    }
                }
            }

            // Back button at bottom left - no border lines
            div style="margin-top: 2rem;" {
                a href=(back_url) style="text-decoration: none;" {
                    button class="button-outline" style="padding: 0.5rem 1rem; font-size: 0.875rem;" { "Back" }
                }
            }
        }
    }
}
