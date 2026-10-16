//! The agent layer's UI (a WordCraft fork feature), plugged in through `wordcraft_ui_egui::ext`:
//!
//! - on the page: questions underlined in their status colour with a badge in the margin
//!   (waiting / the agent is on it / answer ready), the agent's notes as margin cards with a hover
//!   card, client data shaded and tagged with its placeholder;
//! - the Agent pane: ask about the selection, read answers and notes with their sources, mark
//!   client data, pick a client to render a template, the history, how to connect an agent;
//! - the Agent tab on the ribbon (`ribbon.rs`): the pane with what is waiting, client data and the
//!   template's client, the connection (server, agent, Claude Code command, Connect Cowork);
//! - context-menu items: Ask the Agent, Mark as Client Data;
//! - a launcher at the bottom right of the page that says what is waiting.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod overlay;
mod pane;
mod ribbon;

pub use ribbon::request_menu;

use std::sync::{Mutex, OnceLock};

use egui::{Color32, Ui};
use serde_json::{Value as Json, json};
use wordcraft_agent::anchor::{self, Span};
use wordcraft_agent::layer::{Layer, QuestionStatus};
use wordcraft_doc::{Path, Pos, StoryRef};
use wordcraft_engine::{Selection, Session};
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::ext::{PageGeometry, RibbonTab, UiExtension};
use wordcraft_ui_egui::theme::Tokens;

static SERVER: OnceLock<String> = OnceLock::new();

/// Registers the agent layer and its UI (call once, on the UI thread, before the session is
/// made: its command registry is built then).
pub fn install() {
    wordcraft_agent::install();
    wordcraft_ui_egui::ext::register(Box::new(AgentUi::default()));
}

/// Tells the pane where the agent server listens (shown under Connect).
pub fn set_server_url(url: String) {
    let _ = SERVER.set(url);
}

static STDIO_ENTRY: OnceLock<Json> = OnceLock::new();

/// The `claude_desktop_config.json` entry that starts this app in stdio mode (Connect Cowork).
pub fn set_stdio_entry(entry: Json) {
    let _ = STDIO_ENTRY.set(entry);
}

pub(crate) fn stdio_entry() -> Option<Json> {
    STDIO_ENTRY.get().cloned()
}

pub(crate) fn server_url() -> Option<&'static str> {
    SERVER.get().map(String::as_str)
}

static REQUEST: Mutex<Option<(bool, String)>> = Mutex::new(None);

/// Opens (or closes) the Agent pane on a tab (`ask`, `questions`, `notes`, `client`, `history`,
/// `connect`) from the next frame (for scripts and snapshots).
pub fn request_pane(open: bool, tab: &str) {
    if let Ok(mut r) = REQUEST.lock() {
        *r = Some((open, tab.to_string()));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Ask,
    Questions,
    Notes,
    History,
}

impl Tab {
    fn parse(s: &str) -> Tab {
        match s {
            "questions" => Tab::Questions,
            "notes" => Tab::Notes,
            "history" => Tab::History,
            _ => Tab::Ask,
        }
    }
}

/// Something clicked while drawing, acted on in the next `frame` (drawing borrows the app).
#[derive(Clone, Debug)]
pub(crate) enum Act {
    /// A line in the status bar.
    Status(String),
    Open(Tab, Option<String>),
    Follow(String),
    Run(&'static str, Json),
    GoTo(Span),
    /// Ask about a range: the pane's Ask tab with the range chosen.
    AskAbout(Span),
}

#[derive(Default)]
pub(crate) struct AgentUi {
    pub open: bool,
    pub tab: Tab,
    /// Draft question, and whether its box should take focus.
    pub draft: String,
    pub focus_draft: bool,
    /// What the draft is about (else the selection).
    pub ask_span: Option<Span>,
    /// A question or note to bring forward in its tab.
    pub highlight: Option<String>,
    /// A label typed for the next client-data mark.
    pub label: String,
    pub acts: Vec<Act>,
}

/// The agent layer's colours (not in the theme: they mean the same in light and dark).
pub(crate) struct Palette {
    pub pending: Color32,
    pub working: Color32,
    pub answered: Color32,
    pub seen: Color32,
    pub note: Color32,
    pub private: Color32,
    pub agent: Color32,
    pub person: Color32,
}

pub(crate) fn palette(t: &Tokens) -> Palette {
    Palette {
        pending: Color32::from_rgb(0xD9, 0x7A, 0x00),
        working: Color32::from_rgb(0x1F, 0x6F, 0xD1),
        answered: Color32::from_rgb(0x1E, 0x9E, 0x5A),
        seen: t.text_disabled,
        note: Color32::from_rgb(0x7B, 0x4F, 0xD6),
        private: Color32::from_rgb(0xB0, 0x3A, 0x8C),
        agent: Color32::from_rgb(0x7B, 0x4F, 0xD6),
        person: t.accent,
    }
}

pub(crate) fn status_text(q: &wordcraft_agent::layer::Question) -> (&'static str, &'static str) {
    match q.status {
        QuestionStatus::Pending => ("○", "Waiting for the agent"),
        QuestionStatus::Working => ("●", "The agent is on it…"),
        QuestionStatus::Answered if !q.seen => ("●", "Answer ready"),
        QuestionStatus::Answered => ("○", "Answered"),
    }
}

pub(crate) fn status_color(q: &wordcraft_agent::layer::Question, p: &Palette) -> Color32 {
    match q.status {
        QuestionStatus::Pending => p.pending,
        QuestionStatus::Working => p.working,
        QuestionStatus::Answered if !q.seen => p.answered,
        QuestionStatus::Answered => p.seen,
    }
}

pub(crate) fn kind_label(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "faq" => ("?", "Prepared answer"),
        "assumption" => ("~", "Assumption"),
        "source" => ("↗", "Source"),
        _ => ("i", "Explanation"),
    }
}

/// Where an anchor is in the document on screen.
pub(crate) fn span_of(s: &Session, paras: &[Path], l: &Layer, anchor_id: &str) -> Option<Span> {
    l.anchor(anchor_id).and_then(|a| anchor::resolve(&s.doc, paras, a))
}

/// The agent's mark: a four-pointed star, drawn (the interface fonts have no such glyph).
pub(crate) fn sparkle(ui: &mut Ui, color: Color32, size: f32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let c = r.center();
    let (o, i) = (size * 0.5, size * 0.14);
    let pts = vec![
        c + egui::vec2(0.0, -o),
        c + egui::vec2(i, -i),
        c + egui::vec2(o, 0.0),
        c + egui::vec2(i, i),
        c + egui::vec2(0.0, o),
        c + egui::vec2(-i, i),
        c + egui::vec2(-o, 0.0),
        c + egui::vec2(-i, -i),
    ];
    ui.painter().add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

/// "3 min ago".
pub(crate) fn ago(at: u64) -> String {
    let now = wordcraft_agent::now_ms();
    if at == 0 || now < at {
        return String::new();
    }
    let s = (now - at) / 1000;
    match s {
        0..=44 => "just now".into(),
        45..=5399 => format!("{} min ago", (s / 60).max(1)),
        5400..=129_599 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

pub(crate) fn clip(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if t.chars().count() < s.chars().count() { format!("{}…", t.trim_end()) } else { t }
}

/// The range a reader's command would apply to now: the selection's first paragraph, or the
/// caret's paragraph.
pub(crate) fn selection_span(s: &Session) -> Option<Span> {
    let (a, b) = s.sel.ordered();
    if a.story != StoryRef::Body {
        return None;
    }
    let paras = anchor::paragraphs(&s.doc);
    let i = paras.iter().position(|p| *p == a.path)?;
    let len = anchor::para_text(&s.doc, &a.path).map_or(0, str::len);
    let (start, end) = if a == b {
        (0, len)
    } else if a.path == b.path {
        (a.off, b.off)
    } else {
        (a.off, len)
    };
    Some(Span { para: i, path: a.path.clone(), start, end })
}

/// The real text of a span.
pub(crate) fn span_text(s: &Session, sp: &Span) -> String {
    anchor::para_text(&s.doc, &sp.path).and_then(|t| t.get(sp.start..sp.end)).unwrap_or("").replace(wordcraft_doc::para::OBJ, "")
}

/// Params naming a span for a reader's command (paragraph + real quote).
pub(crate) fn span_params(s: &Session, sp: &Span) -> Json {
    let text = anchor::para_text(&s.doc, &sp.path).and_then(|t| t.get(sp.start..sp.end)).unwrap_or("");
    let before = anchor::para_text(&s.doc, &sp.path).and_then(|t| t.get(..sp.start)).unwrap_or("");
    let occurrence = if text.is_empty() { 0 } else { before.matches(text).count() };
    if text.is_empty() || sp.start == 0 && anchor::para_text(&s.doc, &sp.path).is_some_and(|t| t.len() == sp.end) {
        json!({"paragraph": sp.para})
    } else {
        json!({"paragraph": sp.para, "quote": text, "occurrence": occurrence})
    }
}

/// Selects a span and scrolls it into view.
pub(crate) fn go_to(app: &mut WordApp, sp: &Span) {
    let a = Pos::new(StoryRef::Body, sp.path.clone(), sp.start);
    let b = Pos::new(StoryRef::Body, sp.path.clone(), sp.end);
    app.session.sel = Selection { anchor: app.session.doc.clamp(&a), focus: app.session.doc.clamp(&b) };
    app.canvas.scroll_to_caret = true;
}

/// Follows a link: `para:12` goes to the paragraph, `note:n3` shows the note, a URL opens.
pub(crate) fn follow(st: &mut AgentUi, app: &mut WordApp, ctx: &egui::Context, href: &str) {
    if let Some(n) = href.strip_prefix("para:").and_then(|n| n.trim().parse::<usize>().ok()) {
        let paras = anchor::paragraphs(&app.session.doc);
        if let Some(path) = paras.get(n) {
            let len = anchor::para_text(&app.session.doc, path).map_or(0, str::len);
            go_to(app, &Span { para: n, path: path.clone(), start: 0, end: len });
        }
    } else if let Some(id) = href.strip_prefix("note:") {
        st.open = true;
        st.tab = Tab::Notes;
        st.highlight = Some(id.to_string());
    } else if href.starts_with("https://") || href.starts_with("http://") {
        ctx.open_url(egui::OpenUrl::new_tab(href));
    }
}

impl AgentUi {
    fn act(&mut self, app: &mut WordApp, ctx: &egui::Context) {
        for a in std::mem::take(&mut self.acts) {
            match a {
                Act::Open(tab, id) => {
                    self.open = true;
                    self.tab = tab;
                    if let Some(id) = id {
                        // Opening an answer marks it read.
                        let unseen = wordcraft_agent::layer(&app.session)
                            .questions
                            .iter()
                            .any(|q| q.id == id && q.status == QuestionStatus::Answered && !q.seen);
                        if unseen {
                            let _ = app.run("agent.seen", json!({"id": id}));
                        }
                        self.highlight = Some(id);
                    }
                }
                Act::Follow(href) => follow(self, app, ctx, &href),
                Act::Status(s) => app.status(s),
                Act::Run(id, params) => {
                    if let Err(e) = app.run(id, params) {
                        app.status(e);
                    }
                }
                Act::GoTo(sp) => go_to(app, &sp),
                Act::AskAbout(sp) => {
                    self.open = true;
                    self.tab = Tab::Ask;
                    self.ask_span = Some(sp);
                    self.focus_draft = true;
                }
            }
        }
    }

    /// The Agent button at the bottom right of the page area, with what is waiting: only while the
    /// ribbon (and its Agent tab) is out of sight.
    fn launcher(&mut self, app: &WordApp, ctx: &egui::Context) {
        let Some(area) = app.canvas.canvas_rect else { return };
        let l = wordcraft_agent::layer(&app.session);
        let ready = l.questions.iter().filter(|q| q.status == QuestionStatus::Answered && !q.seen).count();
        let working = l.questions.iter().filter(|q| q.status == QuestionStatus::Working).count();
        let waiting = l.questions.iter().filter(|q| q.status == QuestionStatus::Pending).count();
        let t = Tokens::get(ctx);
        let p = palette(&t);
        let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("{n} {one}") } else { format!("{n} {many}") };
        let (text, color) = if ready > 0 {
            (format!("●  {} ready", plural(ready, "answer", "answers")), p.answered)
        } else if working > 0 {
            (format!("●  The agent is on {}…", plural(working, "question", "questions")), p.working)
        } else if waiting > 0 {
            (format!("○  {} waiting", plural(waiting, "question", "questions")), p.pending)
        } else {
            ("Agent".to_string(), p.agent)
        };
        if self.open && ready + working + waiting == 0 {
            return;
        }
        let pos = egui::pos2(area.right() - 24.0, area.bottom() - 16.0);
        egui::Area::new(egui::Id::new("agent_launcher")).pivot(egui::Align2::RIGHT_BOTTOM).fixed_pos(pos).order(egui::Order::Foreground).show(
            ctx,
            |ui| {
                let b = egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
                    .fill(color)
                    .corner_radius(14.0)
                    .min_size(egui::vec2(0.0, 28.0))
                    .wrap_mode(egui::TextWrapMode::Extend);
                if ui.add(b).on_hover_text("Questions, answers and notes of your agent").clicked() {
                    let tab = if ready + working + waiting > 0 { Tab::Questions } else { Tab::Ask };
                    self.acts.push(Act::Open(tab, None));
                }
            },
        );
    }
}

impl UiExtension for AgentUi {
    fn frame(&mut self, app: &mut WordApp, ctx: &egui::Context) {
        wordcraft_agent::ensure(&mut app.session);
        // The background agent: started on the first question, fed each new one.
        wordcraft_agent::runner::tick(&mut app.session);
        // AItoCraft: what is open, for the registry (written when it changes, and as a heartbeat).
        if ctx.input(|i| i.focused) {
            wordcraft_agent::note_focus();
        }
        if app.has_document() {
            wordcraft_agent::registry::observe(serde_json::json!([wordcraft_agent::document_json(&app.session)]));
        } else {
            wordcraft_agent::registry::observe(serde_json::json!([]));
        }
        if let Some((open, tab)) = REQUEST.lock().ok().and_then(|mut r| r.take()) {
            self.open = open;
            self.tab = Tab::parse(&tab);
        }
        self.act(app, ctx);
        let ribbon_hidden = app.ui.ribbon_collapsed || app.session.view.focus_mode || app.session.view.read_mode;
        if app.has_document() && !app.ui.backstage && ribbon_hidden {
            self.launcher(app, ctx);
        }
        // Answers arrive from another thread (the control channel): look again soon.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn side_panel(&mut self, app: &mut WordApp, ui: &mut Ui) {
        if self.open {
            pane::show(self, app, ui);
        }
    }

    fn paint_pages(&mut self, app: &WordApp, ui: &Ui, painter: &egui::Painter, pages: &PageGeometry) {
        overlay::paint(self, app, ui, painter, pages);
    }

    fn ribbon_tabs(&self) -> Vec<RibbonTab> {
        vec![RibbonTab { name: ribbon::TAB, keytip: ribbon::KEYTIP }]
    }

    fn ribbon_tab(&mut self, app: &mut WordApp, ui: &mut Ui, name: &str) -> bool {
        if name != ribbon::TAB {
            return false;
        }
        ribbon::show(self, app, ui);
        true
    }

    fn context_menu(&mut self, app: &mut WordApp, ui: &mut Ui) {
        ui.separator();
        let sp = selection_span(&app.session);
        if ui.button("Ask the Agent…").clicked() {
            if let Some(sp) = sp.clone() {
                self.acts.push(Act::AskAbout(sp));
            }
            ui.close();
        }
        let l = wordcraft_agent::layer(&app.session);
        let paras = anchor::paragraphs(&app.session.doc);
        let in_private = sp.as_ref().is_some_and(|sp| {
            l.private.iter().any(|p| {
                span_of(&app.session, &paras, &l, &p.anchor)
                    .is_some_and(|x| x.path == sp.path && x.start < sp.end.max(sp.start + 1) && sp.start < x.end)
            })
        });
        let preview = wordcraft_agent::render_state(&app.session).is_some();
        if !preview && let Some(sp) = sp {
            if in_private {
                if ui.button("Unmark Client Data").clicked() {
                    let mut p = span_params(&app.session, &sp);
                    p["on"] = json!(false);
                    self.acts.push(Act::Run("agent.private", p));
                    ui.close();
                }
            } else if !app.session.sel.is_collapsed() && ui.button("Mark as Client Data").clicked() {
                self.acts.push(Act::Run("agent.private", span_params(&app.session, &sp)));
                ui.close();
            }
        }
        if ui.button(if self.open { "Hide Agent Pane" } else { "Agent Pane" }).clicked() {
            self.open = !self.open;
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use wordcraft_ui_egui::keytips::{self, Phase};
    use wordcraft_ui_egui::{Services, WordApp};

    #[test]
    fn the_agent_tab_has_a_keytip_and_alt_a_opens_it() {
        super::install();
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_agent::sample::engagement_letter()), Services::default());
        assert_eq!(keytips::tab_letter("Agent"), "A");
        assert!(keytips::badge_tabs(&app).contains(&"Agent"));
        // The built-in letters are untouched.
        assert_eq!(keytips::tab_letter("Help"), "Y");
        assert_eq!(keytips::tab_letter("Table Design"), "JT");
        app.ui.keytips = Phase::Tabs;
        assert!(keytips::handle_key(&mut app, egui::Key::A));
        assert_eq!(app.ui.tab, "Agent");
        // Its controls have no badges: a letter there just leaves keytips.
        assert_eq!(app.ui.keytips, Phase::Commands);
        assert!(keytips::handle_key(&mut app, egui::Key::Z));
        assert_eq!(app.ui.keytips, Phase::Off);
    }
}
