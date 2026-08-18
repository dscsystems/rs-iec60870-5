// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Application state, key handling and the client lifecycle.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientOption};

use crate::event::{Event, Sender};
use crate::form::{CMD_KINDS, CmdKind, FormInput};
use crate::handler::{Point, TuiHandler};

/// The maximum number of log lines kept in memory.
const MAX_LOG_LINES: usize = 2000;

/// Which panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Points,
    Log,
    Send,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Points, Tab::Log, Tab::Send];

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn shifted(self, by: isize) -> Tab {
        let n = Tab::ALL.len() as isize;
        Tab::ALL[((self.index() as isize + by).rem_euclid(n)) as usize]
    }
}

/// Where keystrokes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The panel itself: hotkeys act on the connection.
    Main,
    /// Editing the target address and common address.
    Connection,
    /// Editing the command builder.
    Form,
}

/// A single-line text field.
#[derive(Debug, Clone, Default)]
pub struct Field {
    pub value: String,
}

impl Field {
    fn new(value: &str) -> Field {
        Field {
            value: value.to_string(),
        }
    }

    /// Apply a keystroke; returns false when the key was not consumed.
    fn key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.push(c);
                true
            }
            KeyCode::Backspace => {
                self.value.pop();
                true
            }
            _ => false,
        }
    }
}

/// The fields of the command builder, in tab order.
pub const FORM_FIELDS: usize = 5;

/// Everything the UI draws and the keys act on.
pub struct App {
    /// Where messages from the protocol tasks are sent.
    pub tx: Sender,
    /// The running client, if any.
    pub client: Option<Arc<Client<TuiHandler>>>,

    pub addr: String,
    pub common_addr: CommonAddr,
    pub connected: bool,
    pub active: bool,
    /// Whether protocol-level tracing is forwarded to the log panel.
    pub verbose: Arc<AtomicBool>,

    pub tab: Tab,
    pub focus: Focus,
    pub should_quit: bool,

    /// Monitored points, keyed by address so the table stays sorted.
    pub points: BTreeMap<InfoObjAddr, Point>,
    /// Which table row is selected.
    pub selected: usize,
    pub logs: Vec<String>,
    /// How far the log panel is scrolled from the bottom.
    pub log_scroll: u16,

    // command builder
    pub form_kind: usize,
    pub form_field: usize,
    pub form_select: bool,
    pub in_ioa: Field,
    pub in_value: Field,
    pub in_qualifier: Field,

    // connection editor
    pub conn_field: usize,
    pub conn_addr: Field,
    pub conn_ca: Field,
}

impl App {
    pub fn new(tx: Sender, verbose: Arc<AtomicBool>, addr: Option<String>) -> App {
        let addr = addr.unwrap_or_else(|| "127.0.0.1:2404".to_string());
        App {
            tx,
            client: None,
            conn_addr: Field::new(&addr),
            addr,
            common_addr: 1,
            conn_ca: Field::new("1"),
            connected: false,
            active: false,
            verbose,
            tab: Tab::Points,
            focus: Focus::Main,
            should_quit: false,
            points: BTreeMap::new(),
            selected: 0,
            logs: Vec::new(),
            log_scroll: 0,
            form_kind: 0,
            form_field: 0,
            form_select: false,
            in_ioa: Field::new(""),
            in_value: Field::new(""),
            in_qualifier: Field::new("0"),
            conn_field: 0,
        }
    }

    pub fn kind(&self) -> CmdKind {
        CMD_KINDS[self.form_kind]
    }

    // -- messages from the protocol tasks ---------------------------------

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Log(line) => self.log(line),
            Event::Points(points) => self.apply_points(points),
            Event::Status {
                connected,
                active,
                note,
            } => {
                self.connected = connected;
                self.active = active;
                if let Some(note) = note {
                    self.log(format!("[app] {note}"));
                }
            }
        }
    }

    pub fn log(&mut self, line: impl Into<String>) {
        let stamped = format!(
            "{} {}",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            line.into()
        );
        self.logs.push(stamped);
        if self.logs.len() > MAX_LOG_LINES {
            let excess = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(..excess);
        }
        // Stay pinned to the newest line.
        self.log_scroll = 0;
    }

    fn apply_points(&mut self, points: Vec<Point>) {
        for mut p in points {
            // Count how often each address has been reported, so a live link
            // is visible even when the values do not change.
            p.count = self.points.get(&p.ioa).map_or(1, |old| old.count + 1);
            self.points.insert(p.ioa, p);
        }
    }

    // -- key handling ------------------------------------------------------

    pub fn key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        match self.focus {
            Focus::Connection => self.key_connection(key),
            Focus::Form => self.key_form(key),
            Focus::Main => self.key_main(key),
        }
    }

    fn key_main(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') => self.tab = Tab::Points,
            KeyCode::Char('2') => self.tab = Tab::Log,
            KeyCode::Char('3') => self.tab = Tab::Send,
            KeyCode::Tab => self.tab = self.tab.shifted(1),
            KeyCode::BackTab => self.tab = self.tab.shifted(-1),

            KeyCode::Char('e') => {
                self.conn_addr = Field::new(&self.addr);
                self.conn_ca = Field::new(&self.common_addr.to_string());
                self.conn_field = 0;
                self.focus = Focus::Connection;
            }
            KeyCode::Char('c') => self.connect(),
            KeyCode::Char('x') => self.disconnect(),

            KeyCode::Char('v') => {
                let on = !self.verbose.load(Ordering::Relaxed);
                self.verbose.store(on, Ordering::Relaxed);
                self.log(format!("[app] protocol logging {}", if on { "on" } else { "off" }));
            }

            KeyCode::Char('s') => self.start_dt(),
            KeyCode::Char('S') => self.stop_dt(),

            KeyCode::Char('g') => self.general_interrogation(),
            KeyCode::Char('C') => self.counter_interrogation(),
            KeyCode::Char('y') => self.clock_sync(),
            KeyCode::Char('t') => self.test_command(),
            KeyCode::Char('z') => self.reset_process(),

            KeyCode::Char('l') if ctrl => {
                self.logs.clear();
                self.log_scroll = 0;
            }
            KeyCode::Char('r') if ctrl => {
                self.points.clear();
                self.selected = 0;
            }

            KeyCode::Char('i') if self.tab == Tab::Send => {
                self.focus = Focus::Form;
                self.form_field = 0;
            }

            // Navigation within the visible panel.
            KeyCode::Up | KeyCode::Char('k') => match self.tab {
                Tab::Points => self.selected = self.selected.saturating_sub(1),
                Tab::Log => self.log_scroll = self.log_scroll.saturating_add(1),
                Tab::Send => {}
            },
            KeyCode::Down | KeyCode::Char('j') => match self.tab {
                Tab::Points => {
                    let last = self.points.len().saturating_sub(1);
                    self.selected = (self.selected + 1).min(last);
                }
                Tab::Log => self.log_scroll = self.log_scroll.saturating_sub(1),
                Tab::Send => {}
            },
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.points.len().saturating_sub(1),
            _ => {}
        }
    }

    fn key_connection(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.focus = Focus::Main,
            KeyCode::Enter => {
                self.addr = self.conn_addr.value.trim().to_string();
                if let Ok(ca) = self.conn_ca.value.trim().parse::<u16>() {
                    self.common_addr = ca;
                }
                self.focus = Focus::Main;
                let (addr, ca) = (self.addr.clone(), self.common_addr);
                self.log(format!("[app] target set: {addr} common address {ca}"));
            }
            KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up => {
                self.conn_field = (self.conn_field + 1) % 2;
            }
            _ => {
                let field = if self.conn_field == 0 {
                    &mut self.conn_addr
                } else {
                    &mut self.conn_ca
                };
                field.key(key);
            }
        }
    }

    fn key_form(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.focus = Focus::Main,
            KeyCode::Enter => self.send_form(),
            KeyCode::Tab | KeyCode::Down => {
                self.form_field = (self.form_field + 1) % FORM_FIELDS;
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.form_field = (self.form_field + FORM_FIELDS - 1) % FORM_FIELDS;
            }
            KeyCode::Left => match self.form_field {
                0 => self.form_kind = (self.form_kind + CMD_KINDS.len() - 1) % CMD_KINDS.len(),
                3 => self.form_select = !self.form_select,
                _ => {}
            },
            KeyCode::Right => match self.form_field {
                0 => self.form_kind = (self.form_kind + 1) % CMD_KINDS.len(),
                3 => self.form_select = !self.form_select,
                _ => {}
            },
            _ => match self.form_field {
                1 => {
                    self.in_ioa.key(key);
                }
                2 => {
                    self.in_value.key(key);
                }
                4 => {
                    self.in_qualifier.key(key);
                }
                _ => {}
            },
        }
    }

    // -- client lifecycle --------------------------------------------------

    fn connect(&mut self) {
        if self.client.is_some() {
            self.disconnect();
        }

        // Automatic reconnection is off: an explorer should show exactly what
        // it was told to do, and 'c' is how you retry.
        let option = match ClientOption::new()
            .with_server(&self.addr)
            .map(|o| o.with_auto_reconnect(false))
        {
            Ok(o) => o,
            Err(e) => {
                self.log(format!("[err] bad server address: {e}"));
                return;
            }
        };

        let client = Client::new(
            TuiHandler {
                tx: self.tx.clone(),
            },
            option,
        );

        self.log(format!("[app] connecting to {}", self.addr));
        if let Err(e) = client.start() {
            self.log(format!("[err] start failed: {e}"));
            return;
        }
        self.client = Some(client);
    }

    fn disconnect(&mut self) {
        if let Some(client) = self.client.take() {
            client.close();
            self.connected = false;
            self.active = false;
            self.log("[app] disconnected");
        }
    }

    /// The client, when one is running; otherwise a note in the log.
    fn client(&mut self) -> Option<Arc<Client<TuiHandler>>> {
        match self.client.clone() {
            Some(c) => Some(c),
            None => {
                self.log("[app] not connected — press 'c' to connect first");
                None
            }
        }
    }

    /// The client, when data transfer is active.
    fn active_client(&mut self) -> Option<Arc<Client<TuiHandler>>> {
        if !self.active {
            self.log("[app] not active — press 'c' to connect, or 's' to send STARTDT");
            return None;
        }
        self.client()
    }

    fn start_dt(&mut self) {
        if let Some(c) = self.client() {
            self.log("[tx] STARTDT act -> sent");
            tokio::spawn(async move { c.send_start_dt().await });
        }
    }

    fn stop_dt(&mut self) {
        if let Some(c) = self.client() {
            self.log("[tx] STOPDT act -> sent");
            tokio::spawn(async move { c.send_stop_dt().await });
        }
    }

    // -- requests ----------------------------------------------------------

    /// Run a request on the client task and report the outcome in the log.
    fn spawn_request<F, Fut>(&mut self, name: &'static str, f: F)
    where
        F: FnOnce(Arc<Client<TuiHandler>>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = rs_iec60870_5::Result<()>> + Send,
    {
        let Some(client) = self.active_client() else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let line = match f(client).await {
                Ok(()) => format!("[tx] {name} -> sent"),
                Err(e) => format!("[tx] {name} failed: {e}"),
            };
            let _ = tx.send(Event::Log(line));
        });
    }

    fn general_interrogation(&mut self) {
        let ca = self.common_addr;
        self.spawn_request("general interrogation", move |c| async move {
            c.interrogation_cmd(
                CauseOfTransmission::new(Cause::ACTIVATION),
                ca,
                QualifierOfInterrogation::STATION,
            )
            .await
        });
    }

    fn counter_interrogation(&mut self) {
        let ca = self.common_addr;
        self.spawn_request("counter interrogation", move |c| async move {
            c.counter_interrogation_cmd(
                CauseOfTransmission::new(Cause::ACTIVATION),
                ca,
                QualifierCountCall {
                    request: QccRequest::TOTAL,
                    freeze: QccFreeze::READ,
                },
            )
            .await
        });
    }

    fn clock_sync(&mut self) {
        let ca = self.common_addr;
        self.spawn_request("clock synchronization", move |c| async move {
            c.clock_synchronization_cmd(
                CauseOfTransmission::new(Cause::ACTIVATION),
                ca,
                chrono::Utc::now(),
            )
            .await
        });
    }

    fn test_command(&mut self) {
        let ca = self.common_addr;
        self.spawn_request("test command", move |c| async move {
            c.test_command(CauseOfTransmission::new(Cause::ACTIVATION), ca)
                .await
        });
    }

    fn reset_process(&mut self) {
        let ca = self.common_addr;
        self.spawn_request("reset process", move |c| async move {
            c.reset_process_cmd(
                CauseOfTransmission::new(Cause::ACTIVATION),
                ca,
                QualifierOfResetProcessCmd::GENERAL_RESET,
            )
            .await
        });
    }

    /// Build the command the form describes and queue it.
    fn send_form(&mut self) {
        let Some(client) = self.client() else { return };

        let kind = self.kind();
        let input = FormInput {
            kind,
            ioa_text: &self.in_ioa.value,
            value_text: &self.in_value.value,
            qualifier_text: &self.in_qualifier.value,
            select: self.form_select,
            common_addr: self.common_addr,
            params: client.params(),
        };

        let asdu = match crate::form::build(&input) {
            Ok(a) => a,
            Err(e) => {
                self.log(format!("[app] {e}"));
                return;
            }
        };

        let summary = format!(
            "[tx] {} IOA={} value={:?} mode={}",
            kind.name,
            self.in_ioa.value.trim(),
            self.in_value.value,
            if self.form_select { "select" } else { "execute" }
        );

        let tx = self.tx.clone();
        tokio::spawn(async move {
            let line = match client.send(asdu).await {
                Ok(()) => format!("{summary} -> queued"),
                Err(e) => format!("{summary} failed: {e}"),
            };
            let _ = tx.send(Event::Log(line));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn app() -> App {
        let (tx, _rx) = crate::event::channel();
        App::new(tx, Arc::new(AtomicBool::new(false)), None)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn number_keys_and_tab_switch_panels() {
        let mut a = app();
        assert_eq!(a.tab, Tab::Points);
        a.key(key(KeyCode::Char('2')));
        assert_eq!(a.tab, Tab::Log);
        a.key(key(KeyCode::Char('3')));
        assert_eq!(a.tab, Tab::Send);
        a.key(key(KeyCode::Tab));
        assert_eq!(a.tab, Tab::Points, "tab wraps around");
        a.key(key(KeyCode::BackTab));
        assert_eq!(a.tab, Tab::Send, "shift-tab wraps the other way");
    }

    #[test]
    fn quitting_is_q_or_ctrl_c() {
        let mut a = app();
        a.key(key(KeyCode::Char('q')));
        assert!(a.should_quit);

        let mut a = app();
        a.key(ctrl('c'));
        assert!(a.should_quit, "ctrl-c quits from any focus");
    }

    #[test]
    fn ctrl_c_quits_even_while_editing() {
        let mut a = app();
        a.focus = Focus::Form;
        a.key(ctrl('c'));
        assert!(a.should_quit);
    }

    #[test]
    fn the_connection_editor_applies_on_enter_and_discards_on_escape() {
        let mut a = app();
        a.key(key(KeyCode::Char('e')));
        assert_eq!(a.focus, Focus::Connection);

        // Retype the address.
        a.conn_addr.value.clear();
        for c in "10.0.0.5:2404".chars() {
            a.key(key(KeyCode::Char(c)));
        }
        a.key(key(KeyCode::Tab));
        a.conn_ca.value.clear();
        a.key(key(KeyCode::Char('7')));
        a.key(key(KeyCode::Enter));

        assert_eq!(a.focus, Focus::Main);
        assert_eq!(a.addr, "10.0.0.5:2404");
        assert_eq!(a.common_addr, 7);

        // Escaping leaves the applied values untouched.
        a.key(key(KeyCode::Char('e')));
        a.conn_addr.value.clear();
        a.key(key(KeyCode::Esc));
        assert_eq!(a.addr, "10.0.0.5:2404");
    }

    #[test]
    fn the_form_cycles_kinds_and_toggles_the_select_bit() {
        let mut a = app();
        a.tab = Tab::Send;
        a.key(key(KeyCode::Char('i')));
        assert_eq!(a.focus, Focus::Form);
        assert_eq!(a.form_field, 0);

        a.key(key(KeyCode::Right));
        assert_eq!(a.form_kind, 1);
        a.key(key(KeyCode::Left));
        assert_eq!(a.form_kind, 0);
        // Wraps backwards past the first entry.
        a.key(key(KeyCode::Left));
        assert_eq!(a.form_kind, CMD_KINDS.len() - 1);

        // Field 3 is the execute/select toggle.
        a.form_field = 3;
        assert!(!a.form_select);
        a.key(key(KeyCode::Right));
        assert!(a.form_select);
        a.key(key(KeyCode::Left));
        assert!(!a.form_select);
    }

    #[test]
    fn form_typing_reaches_the_focused_field_only() {
        let mut a = app();
        a.focus = Focus::Form;

        a.form_field = 1;
        for c in "6000".chars() {
            a.key(key(KeyCode::Char(c)));
        }
        assert_eq!(a.in_ioa.value, "6000");
        assert!(a.in_value.value.is_empty());

        a.form_field = 2;
        for c in "on".chars() {
            a.key(key(KeyCode::Char(c)));
        }
        assert_eq!(a.in_value.value, "on");

        a.key(key(KeyCode::Backspace));
        assert_eq!(a.in_value.value, "o");
    }

    #[test]
    fn points_are_kept_sorted_and_counted() {
        let mut a = app();
        let point = |ioa: InfoObjAddr, value: &str| Point {
            ioa,
            type_name: "M_SP_NA_1".into(),
            value: value.into(),
            quality: "Good".into(),
            cause: "Spontaneous".into(),
            time: String::new(),
            count: 0,
        };

        a.apply(Event::Points(vec![point(400, "on"), point(100, "off")]));
        let addresses: Vec<_> = a.points.keys().copied().collect();
        assert_eq!(addresses, vec![100, 400], "rows stay in address order");
        assert_eq!(a.points[&100].count, 1);

        // Reporting the same address again updates it and counts the report.
        a.apply(Event::Points(vec![point(100, "on")]));
        assert_eq!(a.points[&100].count, 2);
        assert_eq!(a.points[&100].value, "on");
        assert_eq!(a.points.len(), 2);
    }

    #[test]
    fn the_log_is_stamped_and_bounded() {
        let mut a = app();
        for i in 0..(MAX_LOG_LINES + 50) {
            a.log(format!("line {i}"));
        }
        assert_eq!(a.logs.len(), MAX_LOG_LINES);
        assert!(
            a.logs.last().unwrap().ends_with(&format!("line {}", MAX_LOG_LINES + 49)),
            "the newest line is kept"
        );
        assert!(
            a.logs[0].contains("line 50"),
            "the oldest lines are dropped: {}",
            a.logs[0]
        );
    }

    #[test]
    fn ctrl_l_and_ctrl_r_clear_the_panels() {
        let mut a = app();
        a.log("something");
        a.apply(Event::Points(vec![Point {
            ioa: 1,
            type_name: "M_SP_NA_1".into(),
            value: "on".into(),
            quality: "Good".into(),
            cause: "Spontaneous".into(),
            time: String::new(),
            count: 0,
        }]));

        a.key(ctrl('l'));
        assert!(a.logs.is_empty());
        a.key(ctrl('r'));
        assert!(a.points.is_empty());
    }

    #[test]
    fn status_messages_update_the_indicator_and_log() {
        let mut a = app();
        a.apply(Event::status(true, false, "TCP connected"));
        assert!(a.connected && !a.active);
        assert!(a.logs[0].contains("TCP connected"));

        a.apply(Event::status(true, true, "data transfer active"));
        assert!(a.active);
    }

    #[test]
    fn requests_are_refused_while_inactive_rather_than_panicking() {
        let mut a = app();
        for k in ['g', 'C', 'y', 't', 'z'] {
            a.key(key(KeyCode::Char(k)));
        }
        assert_eq!(a.logs.len(), 5);
        assert!(a.logs.iter().all(|l| l.contains("not active")));
    }

    #[test]
    fn point_navigation_stays_inside_the_table() {
        let mut a = app();
        // Empty table: neither direction moves.
        a.key(key(KeyCode::Down));
        assert_eq!(a.selected, 0);
        a.key(key(KeyCode::Up));
        assert_eq!(a.selected, 0);

        for ioa in [1u32, 2, 3] {
            a.apply(Event::Points(vec![Point {
                ioa,
                type_name: "M_SP_NA_1".into(),
                value: "on".into(),
                quality: "Good".into(),
                cause: "Spontaneous".into(),
                time: String::new(),
                count: 0,
            }]));
        }
        a.key(key(KeyCode::End));
        assert_eq!(a.selected, 2);
        a.key(key(KeyCode::Down));
        assert_eq!(a.selected, 2, "cannot move past the last row");
        a.key(key(KeyCode::Home));
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn verbose_logging_toggles() {
        let mut a = app();
        assert!(!a.verbose.load(Ordering::Relaxed));
        a.key(key(KeyCode::Char('v')));
        assert!(a.verbose.load(Ordering::Relaxed));
        assert!(a.logs.last().unwrap().contains("protocol logging on"));
        a.key(key(KeyCode::Char('v')));
        assert!(!a.verbose.load(Ordering::Relaxed));
    }
}
