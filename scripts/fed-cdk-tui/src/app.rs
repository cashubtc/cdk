use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arboard::Clipboard;
use cdk_common::federation::MemberId;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::telemetry::MemberProbe;
use crate::wallet::{JoinSource, WalletRuntime};

const MAX_ACTIVITIES: usize = 300;
const MAX_LATENCY_SAMPLES: usize = 36;

#[derive(Debug)]
pub enum AppEvent {
    Terminal(Event),
    Joined(Result<Arc<WalletRuntime>, String>),
    MemberProbe(MemberProbe),
    Balance {
        available: u64,
        pending: u64,
        reserved: u64,
    },
    OperationPayload {
        operation: &'static str,
        title: String,
        content: String,
        message: String,
    },
    OperationProgress {
        operation: &'static str,
        message: String,
    },
    OperationFinished {
        operation: &'static str,
        message: String,
    },
    OperationFailed {
        operation: &'static str,
        error: String,
    },
    BackgroundError(String),
}

#[derive(Debug)]
pub enum AppCommand {
    Quit,
    Join(JoinSource),
    Mint(u64),
    Melt(String),
    Send(u64),
    Receive(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Members,
    Activity,
}

impl Tab {
    pub const ALL: [Self; 3] = [Self::Overview, Self::Members, Self::Activity];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Members => "Members",
            Self::Activity => "Activity",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemberState {
    pub member_id: MemberId,
    pub url: String,
    pub online: bool,
    pub metadata_matches: bool,
    pub http_status: Option<u16>,
    pub latency_ms: u64,
    pub latency_history: VecDeque<u64>,
    pub request_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub response_bytes: u64,
    pub active_keysets: usize,
    pub mint_name: Option<String>,
    pub version: Option<String>,
    pub advertised_member_id: Option<u16>,
    pub advertised_federation_id: Option<String>,
    pub advertised_config_digest: Option<String>,
    pub last_error: Option<String>,
    pub observed_at: u64,
}

impl MemberState {
    fn new(member_id: MemberId, url: String) -> Self {
        Self {
            member_id,
            url,
            online: false,
            metadata_matches: false,
            http_status: None,
            latency_ms: 0,
            latency_history: VecDeque::new(),
            request_count: 0,
            success_count: 0,
            failure_count: 0,
            response_bytes: 0,
            active_keysets: 0,
            mint_name: None,
            version: None,
            advertised_member_id: None,
            advertised_federation_id: None,
            advertised_config_digest: None,
            last_error: None,
            observed_at: 0,
        }
    }

    pub fn success_rate(&self) -> u64 {
        self.success_count
            .saturating_mul(100)
            .checked_div(self.request_count)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct ActivityRecord {
    pub sequence: u64,
    pub at: u64,
    pub kind: &'static str,
    pub member_id: Option<MemberId>,
    pub message: String,
    pub ok: Option<bool>,
    pub latency_ms: Option<u64>,
    pub bytes: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Form(Form),
    Payload { title: String, content: String },
    Help,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub kind: FormKind,
    pub title: &'static str,
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    Join,
    Mint,
    Melt,
    Send,
    Receive,
}

pub struct App {
    data_dir: PathBuf,
    runtime: Option<Arc<WalletRuntime>>,
    pub tab: Tab,
    pub members: BTreeMap<MemberId, MemberState>,
    pub selected_member: usize,
    pub activities: VecDeque<ActivityRecord>,
    pub available: u64,
    pub pending: u64,
    pub reserved: u64,
    pub modal: Option<Modal>,
    pub toast: Option<(String, bool, u16)>,
    pub busy_operation: Option<&'static str>,
    pub last_payload: Option<String>,
    pub animation_tick: u64,
    pub monitor_generation: u64,
    should_quit: bool,
    activity_sequence: u64,
}

impl App {
    pub fn new(data_dir: PathBuf, runtime: Option<Arc<WalletRuntime>>) -> Self {
        let mut app = Self {
            data_dir,
            runtime,
            tab: Tab::Overview,
            members: BTreeMap::new(),
            selected_member: 0,
            activities: VecDeque::new(),
            available: 0,
            pending: 0,
            reserved: 0,
            modal: None,
            toast: None,
            busy_operation: None,
            last_payload: None,
            animation_tick: 0,
            monitor_generation: 0,
            should_quit: false,
            activity_sequence: 0,
        };
        if app.runtime.is_some() {
            app.monitor_generation = 1;
            app.rebuild_members();
            app.push_activity(
                "system",
                None,
                "Wallet opened and member probes started",
                Some(true),
                None,
                None,
            );
        } else {
            app.modal = Some(Modal::Form(join_form()));
        }
        app
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn runtime(&self) -> Option<Arc<WalletRuntime>> {
        self.runtime.clone()
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    pub fn tick(&mut self) {
        self.animation_tick = self.animation_tick.wrapping_add(1);
        if let Some((_, _, ttl)) = &mut self.toast {
            *ttl = ttl.saturating_sub(1);
            if *ttl == 0 {
                self.toast = None;
            }
        }
    }

    pub fn operation_started(&mut self, operation: &'static str, message: &str) {
        self.busy_operation = Some(operation);
        self.modal = None;
        self.push_activity(operation, None, message, None, None, None);
        self.toast = Some((message.to_string(), true, 24));
    }

    pub fn apply_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Terminal(_) => {}
            AppEvent::Joined(result) => match result {
                Ok(runtime) => {
                    self.runtime = Some(runtime.clone());
                    self.monitor_generation = self.monitor_generation.saturating_add(1);
                    self.rebuild_members();
                    self.busy_operation = None;
                    self.modal = None;
                    self.push_activity(
                        "join",
                        None,
                        &format!("Joined federation {}", short(&runtime.federation_id(), 14)),
                        Some(true),
                        None,
                        None,
                    );
                    self.toast = Some(("Federation joined".to_string(), true, 32));
                }
                Err(error) => self.operation_failed("join", error),
            },
            AppEvent::MemberProbe(probe) => self.apply_probe(probe),
            AppEvent::Balance {
                available,
                pending,
                reserved,
            } => {
                self.available = available;
                self.pending = pending;
                self.reserved = reserved;
            }
            AppEvent::OperationPayload {
                operation,
                title,
                content,
                message,
            } => {
                self.last_payload = Some(content.clone());
                self.modal = Some(Modal::Payload { title, content });
                self.push_activity(operation, None, &message, None, None, None);
                self.toast = Some(("Payload ready; press c to copy".to_string(), true, 32));
            }
            AppEvent::OperationProgress { operation, message } => {
                self.push_activity(operation, None, &message, None, None, None);
                self.toast = Some((message, true, 24));
            }
            AppEvent::OperationFinished { operation, message } => {
                self.busy_operation = None;
                self.push_activity(operation, None, &message, Some(true), None, None);
                self.toast = Some((message, true, 36));
            }
            AppEvent::OperationFailed { operation, error } => {
                self.operation_failed(operation, error);
            }
            AppEvent::BackgroundError(error) => {
                self.push_activity("system", None, &error, Some(false), None, None);
            }
        }
    }

    fn operation_failed(&mut self, operation: &'static str, error: String) {
        self.busy_operation = None;
        self.push_activity(operation, None, &error, Some(false), None, None);
        self.toast = Some((error, false, 48));
    }

    fn apply_probe(&mut self, probe: MemberProbe) {
        if probe.generation != self.monitor_generation {
            return;
        }
        let Some(member) = self.members.get_mut(&probe.member_id) else {
            return;
        };
        member.online = probe.success;
        member.metadata_matches = probe.metadata_matches;
        member.http_status = probe.status;
        member.latency_ms = probe.latency_ms;
        member.request_count = member.request_count.saturating_add(2);
        member.response_bytes = member
            .response_bytes
            .saturating_add(probe.response_bytes as u64);
        if probe.success {
            member.success_count = member.success_count.saturating_add(2);
        } else {
            member.failure_count = member.failure_count.saturating_add(2);
        }
        member.latency_history.push_back(probe.latency_ms);
        if member.latency_history.len() > MAX_LATENCY_SAMPLES {
            member.latency_history.pop_front();
        }
        member.active_keysets = probe.active_keysets;
        member.mint_name = probe.mint_name;
        member.version = probe.version;
        member.advertised_member_id = probe.advertised_member_id;
        member.advertised_federation_id = probe.advertised_federation_id;
        member.advertised_config_digest = probe.advertised_config_digest;
        member.last_error = probe.error.clone();
        member.observed_at = probe.observed_at;

        let message = match &probe.error {
            Some(error) => format!("GET info + keysets failed: {error}"),
            None => format!(
                "GET info + keysets -> {} in {}ms",
                probe.status.unwrap_or_default(),
                probe.latency_ms
            ),
        };
        self.push_activity(
            "probe",
            Some(probe.member_id),
            &message,
            Some(probe.success),
            Some(probe.latency_ms),
            Some(probe.response_bytes),
        );
    }

    fn rebuild_members(&mut self) {
        self.members.clear();
        let Some(runtime) = &self.runtime else {
            return;
        };
        for (member_id, member) in runtime.public_config().members {
            self.members.insert(
                member_id,
                MemberState::new(member_id, member.public_mint_url.to_string()),
            );
        }
        self.selected_member = 0;
    }

    fn push_activity(
        &mut self,
        kind: &'static str,
        member_id: Option<MemberId>,
        message: &str,
        ok: Option<bool>,
        latency_ms: Option<u64>,
        bytes: Option<usize>,
    ) {
        self.activity_sequence = self.activity_sequence.saturating_add(1);
        self.activities.push_front(ActivityRecord {
            sequence: self.activity_sequence,
            at: unix_time(),
            kind,
            member_id,
            message: message.to_string(),
            ok,
            latency_ms,
            bytes,
        });
        self.activities.truncate(MAX_ACTIVITIES);
    }

    pub fn handle_terminal_event(&mut self, event: Event) -> Vec<AppCommand> {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_key(key)
            }
            Event::Paste(value) => {
                if let Some(Modal::Form(form)) = &mut self.modal {
                    form.value.push_str(value.trim());
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<AppCommand> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return vec![AppCommand::Quit];
        }

        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }

        match key.code {
            KeyCode::Char('q') => return vec![AppCommand::Quit],
            KeyCode::Char('1') => self.tab = Tab::Overview,
            KeyCode::Char('2') => self.tab = Tab::Members,
            KeyCode::Char('3') => self.tab = Tab::Activity,
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('f') if self.runtime.is_none() => {
                self.modal = Some(Modal::Form(join_form()));
            }
            KeyCode::Char('m') if self.can_start_operation() => {
                self.modal = Some(Modal::Form(Form {
                    kind: FormKind::Mint,
                    title: "Mint eCash",
                    label: "Amount (sat)",
                    value: String::new(),
                    hint: "Creates a BOLT11 quote, then waits for threshold signatures",
                }));
            }
            KeyCode::Char('p') if self.can_start_operation() => {
                self.modal = Some(Modal::Form(Form {
                    kind: FormKind::Melt,
                    title: "Pay invoice",
                    label: "BOLT11 invoice",
                    value: String::new(),
                    hint: "Paste an invoice; the federation will quote and execute the melt",
                }));
            }
            KeyCode::Char('s') if self.can_start_operation() => {
                self.modal = Some(Modal::Form(Form {
                    kind: FormKind::Send,
                    title: "Send eCash",
                    label: "Amount (sat)",
                    value: String::new(),
                    hint: "Online exact send; CDK swaps first when exact proofs are unavailable",
                }));
            }
            KeyCode::Char('r') if self.can_start_operation() => {
                self.modal = Some(Modal::Form(Form {
                    kind: FormKind::Receive,
                    title: "Receive eCash",
                    label: "Cashu token",
                    value: String::new(),
                    hint: "Paste a token issued by the imported federation",
                }));
            }
            KeyCode::Char('c') => self.copy_last_payload(),
            KeyCode::Down | KeyCode::Char('j') => self.select_next_member(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous_member(),
            _ => {}
        }
        Vec::new()
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> Vec<AppCommand> {
        match &mut self.modal {
            Some(Modal::Help) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?')) {
                    self.modal = None;
                }
            }
            Some(Modal::Payload { .. }) => match key.code {
                KeyCode::Esc | KeyCode::Enter => self.modal = None,
                KeyCode::Char('c') => self.copy_last_payload(),
                _ => {}
            },
            Some(Modal::Form(form)) => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Backspace => {
                    form.value.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    form.value.clear();
                }
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    form.value.push(character);
                }
                KeyCode::Enter => return self.submit_form(),
                _ => {}
            },
            None => {}
        }
        Vec::new()
    }

    fn submit_form(&mut self) -> Vec<AppCommand> {
        let Some(Modal::Form(form)) = self.modal.take() else {
            return Vec::new();
        };
        let value = form.value.trim().to_string();
        if value.is_empty() {
            self.toast = Some(("Input cannot be empty".to_string(), false, 28));
            self.modal = Some(Modal::Form(form));
            return Vec::new();
        }

        let command = match form.kind {
            FormKind::Join => AppCommand::Join(JoinSource::parse(value)),
            FormKind::Mint => match positive_amount(&value) {
                Ok(amount) => AppCommand::Mint(amount),
                Err(error) => return self.restore_form_with_error(form, error),
            },
            FormKind::Melt => AppCommand::Melt(value),
            FormKind::Send => match positive_amount(&value) {
                Ok(amount) => AppCommand::Send(amount),
                Err(error) => return self.restore_form_with_error(form, error),
            },
            FormKind::Receive => AppCommand::Receive(value),
        };
        vec![command]
    }

    fn restore_form_with_error(&mut self, form: Form, error: String) -> Vec<AppCommand> {
        self.toast = Some((error, false, 28));
        self.modal = Some(Modal::Form(form));
        Vec::new()
    }

    fn can_start_operation(&self) -> bool {
        self.runtime.is_some() && self.busy_operation.is_none()
    }

    fn copy_last_payload(&mut self) {
        let Some(payload) = &self.last_payload else {
            self.toast = Some((
                "No invoice or token is available to copy".to_string(),
                false,
                24,
            ));
            return;
        };
        match Clipboard::new().and_then(|mut clipboard| clipboard.set_text(payload.clone())) {
            Ok(()) => self.toast = Some(("Copied to clipboard".to_string(), true, 24)),
            Err(error) => {
                self.toast = Some((format!("Clipboard unavailable: {error}"), false, 36));
            }
        }
    }

    fn select_next_member(&mut self) {
        if !self.members.is_empty() {
            self.selected_member = (self.selected_member + 1) % self.members.len();
        }
    }

    fn select_previous_member(&mut self) {
        if !self.members.is_empty() {
            self.selected_member = self
                .selected_member
                .checked_sub(1)
                .unwrap_or(self.members.len() - 1);
        }
    }

    pub fn selected_member(&self) -> Option<&MemberState> {
        self.members.values().nth(self.selected_member)
    }

    pub fn federation_id(&self) -> Option<String> {
        self.runtime.as_ref().map(|runtime| runtime.federation_id())
    }

    pub fn local_mint_url(&self) -> Option<String> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.local_mint_url().to_string())
    }

    pub fn threshold_summary(&self) -> Option<(u16, u16, u16)> {
        self.runtime.as_ref().map(|runtime| {
            let params = runtime.public_config().threshold_params;
            (
                params.member_count,
                params.signature_threshold,
                params.consensus_threshold,
            )
        })
    }

    pub fn healthy_members(&self) -> usize {
        self.members.values().filter(|member| member.online).count()
    }

    pub fn total_requests(&self) -> u64 {
        self.members
            .values()
            .map(|member| member.request_count)
            .sum()
    }

    pub fn total_bytes(&self) -> u64 {
        self.members
            .values()
            .map(|member| member.response_bytes)
            .sum()
    }
}

fn join_form() -> Form {
    Form {
        kind: FormKind::Join,
        title: "Join federation",
        label: "Member URL, public config path, or cashufed invite",
        value: String::new(),
        hint: "The public roster is fully validated before any wallet is created",
    }
}

fn positive_amount(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| "Amount must be a positive integer".to_string())
        .and_then(|amount| {
            (amount > 0)
                .then_some(amount)
                .ok_or_else(|| "Amount must be greater than zero".to_string())
        })
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub fn short(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let prefix: String = value.chars().take(max.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_amount_accepts_non_zero_integer() {
        assert_eq!(positive_amount("42"), Ok(42));
    }

    #[test]
    fn positive_amount_rejects_zero_and_non_integer() {
        assert_eq!(
            positive_amount("0"),
            Err("Amount must be greater than zero".to_string())
        );
        assert_eq!(
            positive_amount("1.5"),
            Err("Amount must be a positive integer".to_string())
        );
    }

    #[test]
    fn control_c_quits_even_when_a_modal_is_open() {
        let mut app = App::new(PathBuf::from("test-wallet"), None);
        let commands = app.handle_terminal_event(Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));

        assert!(matches!(commands.as_slice(), [AppCommand::Quit]));
    }
}
