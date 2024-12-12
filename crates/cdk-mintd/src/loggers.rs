use chrono::Utc;
use elasticsearch::http::transport::Transport;
use elasticsearch::{Elasticsearch, IndexParts};
use elasticsearch::auth::Credentials;
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
    pub fn new(elasticsearch_url: &str, index: &str) -> Self {
        let (sender, mut receiver) = mpsc::channel(BACKLOG);
        let client = Elasticsearch::new(
            Transport::single_node(elasticsearch_url)
                .unwrap(),
        );
        let index = index.to_string();

        tokio::spawn(async move {
            while let Some(log) = receiver.recv().await {
                let log_json = serde_json::to_value(log).unwrap();
                if let Err(e) = client
                    .index(IndexParts::Index(&index))
                    .body(log_json)
                    .send()
                    .await
                {
                    eprintln!("Failed to send log to Elasticsearch: {:?}", e);
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
        
        // Manually process the event fields
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
