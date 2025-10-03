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
