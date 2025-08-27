use maud::{html, Markup};

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
