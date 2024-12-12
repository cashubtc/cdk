use chrono::Utc;
use reqwest::Client;
use serde::Serialize;
use tokio::sync::mpsc::{self, Sender};
use tracing::Event;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;
use std::fmt::Write;

pub const BACKLOG: usize = 10_000;

struct Visitor<'a> {
    output: &'a mut String,
}

impl<'a> Visit for Visitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.output, "{}={:?} ", field.name(), value);
    }
}

#[derive(Serialize)]
struct LogMessage {
    timestamp: String,
    level: String,
    message: String,
    target: String,
}

pub struct ElasticsearchLayer {
    sender: Sender<LogMessage>,
}

impl ElasticsearchLayer {
    /// Create a new ElasticsearchLayer.
    ///
    /// # Arguments
    ///
    /// - `elasticsearch_url`: The base URL of the Elasticsearch instance.
    /// - `index`: The name of the index where logs will be sent.
    /// - `api_key`: Optional API key for authentication.
    pub fn new(elasticsearch_url: &str, index: &str, api_key: Option<&str>) -> Self {
        let (sender, mut receiver) = mpsc::channel(BACKLOG);
        let client = Client::new();
        let base_url = format!("{}/{}/_doc", elasticsearch_url.trim_end_matches('/'), index);
        let api_key_header = api_key.map(|key| format!("ApiKey {}", key));

        // Spawn an async task to process logs and send them to Elasticsearch.
        tokio::spawn(async move {
            while let Some(log) = receiver.recv().await {
                let mut request = client
                    .post(&base_url)
                    .json(&log);

                // Add the Authorization header if an API key is provided.
                if let Some(ref key) = api_key_header {
                    request = request.header("Authorization", key);
                }

                let response = request.send().await;

                match response {
                    Ok(res) if !res.status().is_success() => {
                        eprintln!(
                            "Failed to send log to Elasticsearch: HTTP {}",
                            res.status()
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("Failed to send log to Elasticsearch: {:?}", e);
                    }
                }
            }
        });

        Self { sender }
    }
}

impl<S> Layer<S> for ElasticsearchLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let timestamp = Utc::now().to_rfc3339();
        let level = event.metadata().level().to_string();
        let target = event.metadata().target().to_string();
        
        let mut message = String::new();
        let mut visitor = Visitor { output: &mut message };
        event.record(&mut visitor);

        let log = LogMessage {
            timestamp,
            level,
            message,
            target,
        };

        // Non-blocking send to the channel
        let _ = self.sender.try_send(log);
    }
}
