//! The Agent tab on the ribbon: what isn't about one passage. Groups (the same in GridCraft and
//! DeckCraft):
//!
//! - **Agent**: opens the pane, and says what is waiting (answers ready, questions with the agent).
//! - **Client data**: mark or unmark the selection, the marks in the document, and for a template
//!   the client it is shown for.
//! - **Context**: what the agent knows (No context, Claude Cowork, Claude Code), whether it is
//!   ready (the lamp), and Get ready, the steps that make it so.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Ui, pos2, vec2};
use serde_json::json;
use wordcraft_agent::aitocraft;
use wordcraft_agent::anchor;
use wordcraft_agent::context::{self, CodeWay, Context, Kind};
use wordcraft_agent::cowork::{self, Configured, State};
use wordcraft_agent::layer::QuestionStatus;
use wordcraft_agent::mask;
use wordcraft_agent::presence;
use wordcraft_agent::runner;
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::theme::{Tokens, regular};
use wordcraft_ui_egui::widgets::{CONTENT_H, group};

use crate::{Act, AgentUi, Tab, ago, clip, palette, selection_span, span_of, span_params};

/// The tab's name and keytip.
pub(crate) const TAB: &str = "Agent";
pub(crate) const KEYTIP: &str = "A";

/// A large ribbon button drawn like the built-in ones: an icon over one or two label lines.
fn big(
    ui: &mut Ui,
    label: &str,
    sub: Option<(&str, Color32)>,
    checked: bool,
    enabled: bool,
    tip: &str,
    icon: impl FnOnce(&egui::Painter, Rect, Color32),
) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = regular(11.5);
    let w_of = |s: &str| ui.ctx().fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), t.text).size().x);
    let w = label.split('\n').map(w_of).fold(sub.map_or(0.0, |(s, _)| w_of(s)), f32::max).max(32.0) + 12.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w.max(44.0), CONTENT_H), if enabled { Sense::click() } else { Sense::hover() });
    if checked {
        ui.painter().rect_filled(r, CornerRadius::same(4), t.checked);
    } else if resp.hovered() && enabled {
        ui.painter().rect_filled(r, CornerRadius::same(4), t.hover);
    }
    let color = if enabled { t.icon } else { t.text_disabled };
    icon(ui.painter(), Rect::from_center_size(pos2(r.center().x, r.min.y + 19.0), vec2(30.0, 30.0)), color);
    let mut y = r.min.y + 38.0;
    for l in label.split('\n') {
        ui.painter().text(pos2(r.center().x, y), Align2::CENTER_TOP, l, font.clone(), if enabled { t.text } else { t.text_disabled });
        y += 13.0;
    }
    if let Some((s, c)) = sub {
        ui.painter().text(pos2(r.center().x, y), Align2::CENTER_TOP, s, regular(10.5), c);
    }
    resp.on_hover_text(tip)
}

/// A small button with a built-in icon and a label.
fn small(ui: &mut Ui, icon: &str, label: &str, enabled: bool, tip: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let tw = ui.ctx().fonts_mut(|f| f.layout_no_wrap(label.to_string(), regular(11.5), t.text).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2(30.0 + tw, 22.0), if enabled { Sense::click() } else { Sense::hover() });
    if resp.hovered() && enabled {
        ui.painter().rect_filled(r, CornerRadius::same(4), t.hover);
    }
    let (c, a) = if enabled { (t.icon, t.accent) } else { (t.text_disabled, t.text_disabled) };
    wordcraft_ui_egui::icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(17.0, 17.0)), icon, c, a);
    ui.painter().text(pos2(r.min.x + 25.0, r.center().y), Align2::LEFT_CENTER, label, regular(11.5), if enabled { t.text } else { t.text_disabled });
    resp.on_hover_text(tip)
}

/// The agent's mark, drawn (the interface fonts have no such glyph).
fn sparkle(p: &egui::Painter, r: Rect, color: Color32) {
    let c = r.center();
    let (o, i) = (r.width() * 0.42, r.width() * 0.11);
    let pts = vec![
        c + vec2(0.0, -o),
        c + vec2(i, -i),
        c + vec2(o, 0.0),
        c + vec2(i, i),
        c + vec2(0.0, o),
        c + vec2(-i, i),
        c + vec2(-o, 0.0),
        c + vec2(-i, -i),
    ];
    p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    let s = r.width() * 0.16;
    let c2 = c + vec2(o * 0.95, -o * 0.75);
    p.add(egui::Shape::convex_polygon(
        vec![
            c2 + vec2(0.0, -s),
            c2 + vec2(s * 0.3, -s * 0.3),
            c2 + vec2(s, 0.0),
            c2 + vec2(s * 0.3, s * 0.3),
            c2 + vec2(0.0, s),
            c2 + vec2(-s * 0.3, s * 0.3),
            c2 + vec2(-s, 0.0),
            c2 + vec2(-s * 0.3, -s * 0.3),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

/// What the Agent button says under its label.
pub(crate) fn status(app: &WordApp, t: &Tokens) -> (String, Color32) {
    let p = palette(t);
    let l = wordcraft_agent::layer(&app.session);
    let live = || l.questions.iter().filter(|q| !q.hidden);
    let ready = live().filter(|q| q.status == QuestionStatus::Answered && !q.seen).count();
    let working = live().filter(|q| q.status == QuestionStatus::Working).count();
    let waiting = live().filter(|q| q.status == QuestionStatus::Pending).count();
    let so = wordcraft_agent::topic_cmds::sign_off(&app.session);
    let decided = so["agreed"].as_u64().unwrap_or(0);
    let total = decided + so["revise"].as_u64().unwrap_or(0) + so["open"].as_u64().unwrap_or(0);
    if ready > 0 {
        (format!("{ready} ready"), p.answered)
    } else if working > 0 {
        (format!("{working} in work"), p.working)
    } else if waiting > 0 {
        (format!("{waiting} waiting"), p.pending)
    } else if total > 0 {
        // Readiness for sign-off.
        (format!("{decided}/{total} agreed"), if decided == total { p.answered } else { t.text_dim })
    } else {
        ("idle".into(), t.text_disabled)
    }
}

pub(crate) fn show(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let p = palette(&t);
    let (sub, sub_color) = status(app, &t);
    let open = st.open;
    let mut acts: Vec<Act> = Vec::new();

    group(ui, "Agent", None, app, |ui, app| {
        let r = big(
            ui,
            "Agent",
            Some((&sub, sub_color)),
            open,
            true,
            "Show or hide the Agent pane: your questions, the agent's answers and notes",
            |pa, rect, _| sparkle(pa, rect, p.agent),
        );
        if r.clicked() {
            if open {
                st.open = false;
            } else {
                acts.push(Act::Open(Tab::Selection, None));
            }
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
            let so = wordcraft_agent::topic_cmds::sign_off(&app.session);
            let line = so["line"].as_str().unwrap_or("").to_string();
            if small(ui, "pin", "Overview", true, &line).clicked() {
                acts.push(Act::Open(Tab::Overview, None));
            }
            if small(
                ui,
                "share",
                "Save copy to send…",
                app.has_document(),
                "A copy of the document with its places, statuses and notes; the discussion stays here unless included",
            )
            .clicked()
            {
                acts.push(Act::SaveToSend(st.include_discussion));
            }
            ui.checkbox(&mut st.include_discussion, egui::RichText::new("include discussion").size(11.0));
        });
    });

    group(ui, "Client data", None, app, |ui, app| {
        let preview = wordcraft_agent::render_state(&app.session).is_some();
        let sp = selection_span(&app.session);
        let l = wordcraft_agent::layer(&app.session);
        let paras = anchor::paragraphs(&app.session.doc);
        let in_private = sp.as_ref().is_some_and(|sp| {
            l.private.iter().any(|pr| {
                span_of(&app.session, &paras, &l, &pr.anchor)
                    .is_some_and(|x| x.path == sp.path && x.start < sp.end.max(sp.start + 1) && sp.start < x.end)
            })
        });
        let selected = !app.session.sel.is_collapsed();
        let (label, can) = if in_private { ("Unmark\nClient Data", !preview) } else { ("Mark as\nClient Data", !preview && selected) };
        let tip = if preview {
            "Show the template (not a client's rendering) to mark client data"
        } else if in_private {
            "Lift the client-data mark: the agent will see this text"
        } else {
            "The agent sees a {{placeholder}} instead of the selected text, everywhere, and can't change it"
        };
        if big(ui, label, None, false, can, tip, |pa, rect, c| wordcraft_ui_egui::icons::paint(pa, rect.shrink(1.0), "protect", c, p.private))
            .clicked()
            && let Some(sp) = sp
        {
            let mut params = span_params(&app.session, &sp);
            if in_private {
                params["on"] = json!(false);
            } else if !st.label.trim().is_empty() {
                params["label"] = json!(st.label.trim());
                st.label.clear();
            }
            acts.push(Act::Run("agent.private", params));
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
            // The placeholder's name for the next mark (else one is made up).
            ui.add_enabled(
                !preview,
                egui::TextEdit::singleline(&mut st.label).hint_text("placeholder, e.g. client.name").desired_width(170.0).font(regular(11.0)),
            )
            .on_hover_text("The name the agent sees instead of the text you mark: {{client.name}}");
            // The marks in the document.
            let marks = small(ui, "bookmark", &format!("Marks ({})", l.private.len()), !l.private.is_empty(), "The client data marked in this document");
            egui::Popup::menu(&marks).show(|ui| {
                for pr in &l.private {
                    let at = span_of(&app.session, &paras, &l, &pr.anchor);
                    let text = l.anchor(&pr.anchor).map(|a| clip(&a.quote, 40)).unwrap_or_default();
                    if ui.button(format!("{}  “{}”", mask::placeholder(&pr.label), text)).clicked() {
                        if let Some(sp) = at {
                            acts.push(Act::GoTo(sp));
                        }
                        ui.close();
                    }
                }
            });
            // A template: the client it is shown for.
            let template = wordcraft_agent::template::is_template(wordcraft_agent::agent_doc(&app.session));
            if template {
                let clients = wordcraft_agent::crm::clients();
                let current = wordcraft_agent::render_state(&app.session).map(|r| r.client.clone());
                let name_of = |id: &str| {
                    if id == wordcraft_agent::copyedit::FILE {
                        return "This file's data".to_string();
                    }
                    clients.iter().find(|c| c["id"] == id).and_then(|c| c["name"].as_str()).unwrap_or(id).to_string()
                };
                let shown = current.as_deref().map_or_else(|| "Template".to_string(), name_of);
                let pick = small(ui, "mergeField", &format!("Show: {}", clip(&shown, 22)), true, "Show the template filled in for a client. Your edits stay in that copy; your agent keeps working on the template and never sees the client's data.");
                egui::Popup::menu(&pick).show(|ui| {
                    if ui.selectable_label(current.is_none(), "The template (placeholders)").clicked() {
                        acts.push(Act::Run("agent.render", json!({"client": null})));
                        ui.close();
                    }
                    let file = wordcraft_agent::copyedit::FILE;
                    if ui
                        .selectable_label(current.as_deref() == Some(file), "This file's data")
                        .on_hover_text("The client data kept in this file: edit a value in the copy and it changes everywhere it is used")
                        .clicked()
                    {
                        acts.push(Act::Run("agent.render", json!({"client": file})));
                        ui.close();
                    }
                    ui.separator();
                    for c in &clients {
                        let id = c["id"].as_str().unwrap_or("");
                        let name = c["name"].as_str().unwrap_or(id);
                        if ui.selectable_label(current.as_deref() == Some(id), format!("{name}  ·  {id}")).clicked() {
                            acts.push(Act::Run("agent.render", json!({"client": id})));
                            ui.close();
                        }
                    }
                });
            } else if small(ui, "documentTemplate", "Demo template", true, "Open the engagement-letter template (placeholders, a loop and a condition)").clicked() {
                acts.push(Act::Run("agent.openDemo", json!({})));
            }
        });
    });

    group(ui, "Context", None, app, |ui, app| context_group(st, ui, app, &t, &mut acts));
    // Voice: ask by speaking and pointing, and the speech model.
    #[cfg(not(target_arch = "wasm32"))]
    group(ui, "Voice", None, app, |ui, _app| voice_group(st, ui, &t, &mut acts));
    st.acts.extend(acts);
}

/// The Voice group: the microphone (status under it) and the model, off until the reader picks one.
#[cfg(not(target_arch = "wasm32"))]
fn voice_group(st: &AgentUi, ui: &mut Ui, t: &Tokens, acts: &mut Vec<Act>) {
    use craft_voice::Model;

    use crate::VoiceAct;
    use crate::voice::{KEY, Status};
    let red = Color32::from_rgb(0xD9, 0x30, 0x25);
    let p = palette(t);
    let v = &st.voice;
    let (status, color, tip): (String, Color32, String) = match (v.meter(), v.status()) {
        (Some((secs, _)), _) => (format!("{secs:.0}s"), red, "Listening. Click the words you mean; click again to send".into()),
        (None, Status::Off) => ("Off".into(), t.text_dim, "Choose a speech model (right) to ask by voice".into()),
        (None, Status::Downloading(done, total)) => {
            let pct = if *total > 0 { *done as f64 * 100.0 / *total as f64 } else { 0.0 };
            (format!("Downloading {pct:.0}%"), p.working, format!("Downloading the speech model ({} of {} MB)", done / 1_000_000, total / 1_000_000))
        }
        (None, Status::Loading) => ("Loading…".into(), p.working, "Loading the speech model".into()),
        (None, Status::Failed(e)) => ("Model error".into(), red, format!("{e}. Choose the model again to retry")),
        (None, Status::Idle | Status::Ready) => (
            // Words, not "⌥": the interface font has no such glyph.
            if cfg!(target_os = "macos") { "Hold Option".to_string() } else { "Hold Alt".to_string() },
            t.text_dim,
            format!("Hold {KEY}, speak, click the words you mean; release to send. Or click here to start and again to send"),
        ),
    };
    let on = v.recording();
    let resp = big(ui, "Ask by\nVoice", Some((&status, color)), on, true, &tip, |pa, r, ink| mic(pa, r, if on { red } else { ink }, on));
    // Download progress as a bar under the status.
    if let Status::Downloading(done, total) = v.status()
        && *total > 0
    {
        let r = resp.rect;
        let bar = Rect::from_min_size(pos2(r.left() + 6.0, r.bottom() - 3.0), vec2(r.width() - 12.0, 2.5));
        ui.painter().rect_filled(bar, 1.0, t.border);
        let k = (*done as f32 / *total as f32).clamp(0.0, 1.0);
        ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * k, bar.height())), 1.0, p.working);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    if resp.clicked() {
        acts.push(Act::Voice(if v.choice().is_some() || v.recording() { VoiceAct::Toggle } else { VoiceAct::Model("small") }));
    }
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
        let here = |m: Model| craft_voice::default_dir().is_some_and(|d| m.is_ready(&d));
        let size = |m: Model| if here(m) { String::new() } else { format!(" · {} MB", m.download_size() / 1_000_000) };
        let mut choice = |label: String, on: bool, tip: &str, id: &'static str| {
            if ui.radio(on, egui::RichText::new(label).size(11.5)).on_hover_text(tip).clicked() && !on {
                acts.push(Act::Voice(VoiceAct::Model(id)));
            }
        };
        choice("Off".into(), v.choice().is_none(), "No voice questions: nothing listens, nothing is downloaded", "off");
        choice(
            format!("Small{}", size(Model::Small)),
            v.choice() == Some(Model::Small),
            "Whisper small, 5-bit: fast (about 1–1.5 s for a 5-second question on an M1 Pro). Downloaded once, on this computer; audio never leaves it",
            "small",
        );
        choice(
            format!("Turbo, slower{}", size(Model::Turbo)),
            v.choice() == Some(Model::Turbo),
            "Whisper large-v3-turbo, 5-bit: more accurate with numbers and names, slower (about 2 s for a 5-second question on an M1 Pro). Downloaded once, on this computer; audio never leaves it",
            "turbo",
        );
    });
}

/// A microphone, drawn (the icon set has none): capsule, cradle, stem and base; filled while
/// recording.
#[cfg(not(target_arch = "wasm32"))]
fn mic(p: &egui::Painter, r: Rect, color: Color32, on: bool) {
    let k = r.width() / 26.0;
    let c = r.center();
    let stroke = egui::Stroke::new(1.6, color);
    let body = Rect::from_center_size(c + vec2(0.0, -4.0 * k), vec2(8.0 * k, 13.0 * k));
    if on {
        p.rect_filled(body, 4.0 * k, color);
    } else {
        p.rect_stroke(body, 4.0 * k, stroke, egui::StrokeKind::Middle);
    }
    let pts: Vec<egui::Pos2> = (0..=16)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 16.0);
            c + vec2(7.0 * k * a.cos(), k * (6.0 * a.sin()))
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
    p.line_segment([c + vec2(0.0, 6.0 * k), c + vec2(0.0, 9.5 * k)], stroke);
    p.line_segment([c + vec2(-4.0 * k, 9.5 * k), c + vec2(4.0 * k, 9.5 * k)], stroke);
}

static MENU_REQUEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Where the Context group's mode button was last drawn (UI tests click it).
static MODE_BUTTON: std::sync::Mutex<Option<Rect>> = std::sync::Mutex::new(None);

#[doc(hidden)]
pub fn mode_button_rect() -> Option<Rect> {
    MODE_BUTTON.lock().ok().and_then(|g| *g)
}

/// Opens one of the Context menus (`context`, `ready`, `more`) on the next frame (for scripts and
/// snapshots); `none` closes them.
pub fn request_menu(name: &str) {
    if let Ok(mut g) = MENU_REQUEST.lock() {
        *g = Some(name.to_string());
    }
}

fn wants_menu(name: &str) -> bool {
    match MENU_REQUEST.lock() {
        Ok(mut g) if g.as_deref() == Some(name) => {
            *g = None;
            true
        }
        _ => false,
    }
}

/// Opens or closes a popup when a script asked for it.
fn scripted(ui: &Ui, name: &str, r: &Response) {
    if wants_menu(name) {
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(r));
    }
}

// ---------------------------------------------------------------- readiness

/// The Ready lamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lamp {
    Ready,
    Waiting,
    NotReady,
}

impl Lamp {
    fn label(self) -> &'static str {
        match self {
            Lamp::Ready => "Ready",
            Lamp::Waiting => "Waiting",
            Lamp::NotReady => "Not ready",
        }
    }
    fn color(self, t: &Tokens) -> Color32 {
        match self {
            Lamp::Ready => Color32::from_rgb(0x2E, 0x9E, 0x4F),
            Lamp::Waiting => Color32::from_rgb(0xE0, 0x9B, 0x1A),
            Lamp::NotReady => t.text_disabled,
        }
    }
}

/// How long "Waiting" lasts after a Get ready step with nobody coming.
const WAIT_MS: u64 = 15 * 60_000;
/// A quiet connection check at most this often.
const QUIET_CHECK_MS: u64 = 120_000;

/// The lamp and what it means (for its tooltip); may ask for a quiet connection check.
pub(crate) fn lamp(st: &mut AgentUi, app: &WordApp, c: &Context, acts: &mut Vec<Act>) -> (Lamp, String) {
    let now = wordcraft_agent::now_ms();
    if st.waiting_since > 0 && now.saturating_sub(st.waiting_since) > WAIT_MS {
        st.waiting_since = 0;
    }
    let waiting = st.waiting_since > 0;
    let not_yet = if waiting { Lamp::Waiting } else { Lamp::NotReady };
    let (lamp, tip) = if c.background() {
        let login = context::login(if waiting { 3_000 } else { 60_000 });
        let folder = c.project_folder();
        if c.kind == Kind::Code && folder.is_none() {
            (not_yet, "Choose the project folder Claude works in (Get ready).".to_string())
        } else if login.at == 0 {
            (not_yet, "Looking for Claude on this computer…".to_string())
        } else if !login.ok {
            (not_yet, format!("{}. Get ready › Log in to Claude.", login.why))
        } else {
            let at = folder.map_or_else(|| "a neutral empty folder: it sees only this document".to_string(), |f| format!("{}", f.display()));
            let web = if c.web_on() { ", with the web" } else { "" };
            (Lamp::Ready, format!("Claude answers in the background, in {at}{web}. It starts with your first question."))
        }
    } else {
        let ping = wordcraft_agent::ping(&app.session);
        let listening = presence::listeners(false);
        let attached = presence::listeners_of(&wordcraft_agent::document_id(&app.session));
        let where_ = if c.kind == Kind::Cowork { "your Cowork chat" } else { "your Claude Code session" };
        match presence::status(ping.as_ref(), &attached, &listening, now) {
            presence::Status::Answering { by, .. } => {
                (Lamp::Ready, if by.is_empty() { format!("Claude is with you in {where_}.") } else { format!("Claude is with you: {by}.") })
            }
            presence::Status::Attached { label, .. } => (Lamp::Ready, format!("Claude is with you: {label}.")),
            presence::Status::Off => (Lamp::NotReady, "The agent connection is switched off.".to_string()),
            other => {
                // Something is connected but hasn't said it is here for this document: ask quietly.
                let out = ping.as_ref().is_some_and(|p| p.answered_at.is_none() && now.saturating_sub(p.asked_at) < presence::PING_TIMEOUT_MS);
                let last = ping.as_ref().map_or(0, |p| p.asked_at);
                if (listening.any() || waiting) && !out && now.saturating_sub(last) > QUIET_CHECK_MS {
                    acts.push(Act::Run("agent.ping", json!({})));
                }
                let tip = match other {
                    presence::Status::Checking | presence::Status::Listener => format!("Waiting for {where_} to join this document."),
                    _ if waiting => format!("Waiting for {where_} to join this document."),
                    _ => format!("Nobody from {where_} is with this document yet (Get ready)."),
                };
                let l = if matches!(other, presence::Status::Checking | presence::Status::Listener) { Lamp::Waiting } else { not_yet };
                (l, tip)
            }
        }
    };
    if lamp == Lamp::Ready {
        st.waiting_since = 0;
    }
    (lamp, tip)
}

// ---------------------------------------------------------------- Get ready

/// One Get ready step.
pub(crate) struct Step {
    pub id: &'static str,
    pub label: &'static str,
    pub done: bool,
    pub tip: String,
}

fn asked_key(app: &WordApp, what: &str) -> String {
    format!("{what}@{}", wordcraft_agent::document_id(&app.session))
}

/// The steps of the context, in order.
pub(crate) fn steps(st: &AgentUi, app: &WordApp, c: &Context, ready: bool) -> Vec<Step> {
    let hub = aitocraft::binary();
    let hub_tip = |what: &str| match &hub {
        Some(_) => what.to_string(),
        None => format!("{what} (AItoCraft isn't on this computer yet: build ~/aitocraft or put `aitocraft` on PATH)"),
    };
    let fast = if st.waiting_since > 0 { 3_000 } else { 60_000 };
    match (c.kind, c.code) {
        (Kind::None, _) => vec![Step {
            id: "login",
            label: "Log in to Claude",
            done: context::login(fast).ok,
            tip: "Opens a terminal with `claude auth login`: the background client uses your Claude subscription".into(),
        }],
        (Kind::Cowork, _) => vec![
            Step {
                id: "install-desktop",
                label: "Install AItoCraft",
                done: context::desktop_hub(),
                tip: hub_tip("Adds AItoCraft to Claude Desktop's settings (then quit Claude with Cmd+Q and open it again)"),
            },
            Step {
                id: "ask-cowork",
                label: "Ask Cowork to collaborate",
                done: ready || st.asked.contains(&asked_key(app, "cowork")),
                tip: "Copies the phrase for Claude: paste it into your Cowork chat".into(),
            },
        ],
        (Kind::Code, CodeWay::Session) => vec![
            Step {
                id: "install-code",
                label: "Install AItoCraft",
                done: context::code_hub(fast).ok,
                tip: hub_tip(&format!("Runs {}", aitocraft::claude_code_command(hub.as_deref()))),
            },
            Step {
                id: "ask-session",
                label: "Ask my session to collaborate",
                done: ready || st.asked.contains(&asked_key(app, "session")),
                tip: format!("Copies the command that starts Claude Code with AItoCraft's channel: {}", aitocraft::CHANNEL_COMMAND),
            },
        ],
        (Kind::Code, CodeWay::Folder) => vec![Step {
            id: "folder",
            label: "Choose folder…",
            done: c.project_folder().is_some(),
            tip: "The folder Claude works in: it sees that folder's CLAUDE.md, memory and files".into(),
        }],
    }
}

static STEP_NOTE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn note(msg: String) {
    if let Ok(mut g) = STEP_NOTE.lock() {
        *g = Some(msg);
    }
}

/// What a step started in the background has to say (shown in the status bar).
pub(crate) fn take_note() -> Option<String> {
    STEP_NOTE.lock().ok().and_then(|mut g| g.take())
}

/// Runs a Get ready step.
pub(crate) fn run_step(st: &mut AgentUi, app: &WordApp, ui: &Ui, id: &str, acts: &mut Vec<Act>) {
    let title = wordcraft_agent::document_title(&app.session);
    match id {
        "login" => match context::log_in() {
            Ok(()) => acts.push(Act::Status("Log in in the terminal that opened; the lamp turns green when you're done".into())),
            Err(e) => acts.push(Act::Status(e)),
        },
        "install-desktop" => {
            aitocraft::install_desktop();
            acts.push(Act::Status("Installing AItoCraft in Claude Desktop… then quit Claude (Cmd+Q) and open it again".into()));
        }
        "ask-cowork" => {
            ui.ctx().copy_text(aitocraft::phrase(&title));
            st.asked.insert(asked_key(app, "cowork"));
            acts.push(Act::Status("Copied: paste it into your Cowork chat".into()));
        }
        "install-code" => {
            let _ = std::thread::Builder::new().name("aitocraft-code-install".into()).spawn(|| {
                note(match context::install_code_hub() {
                    Ok(()) => "AItoCraft is added to Claude Code".into(),
                    Err(e) => e,
                });
            });
            acts.push(Act::Status("Adding AItoCraft to Claude Code…".into()));
        }
        "ask-session" => {
            ui.ctx().copy_text(aitocraft::CHANNEL_COMMAND.to_string());
            st.asked.insert(asked_key(app, "session"));
            acts.push(Act::Status("Copied: run it in a terminal to start Claude Code with AItoCraft's channel".into()));
        }
        "folder" => {
            if st.folder_pick.is_none() {
                let (tx, rx) = std::sync::mpsc::channel();
                let spawned = std::thread::Builder::new().name("agent-folder".into()).spawn(move || {
                    let _ = tx.send(context::choose_folder());
                });
                if spawned.is_ok() {
                    st.folder_pick = Some(rx);
                }
            }
        }
        _ => return,
    }
    st.waiting_since = wordcraft_agent::now_ms();
}

/// A folder chosen in the system's chooser becomes the project folder.
pub(crate) fn poll_folder(st: &mut AgentUi) {
    let Some(rx) = &st.folder_pick else { return };
    match rx.try_recv() {
        Ok(Some(f)) => {
            st.folder_pick = None;
            st.acts.push(Act::Run("agent.setContext", json!({"kind": "code", "code": "folder", "folder": f})));
        }
        Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => st.folder_pick = None,
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
    }
}

// ---------------------------------------------------------------- the group

/// A chip: what the agent knows.
fn chip(ui: &mut Ui, text: &str, t: &Tokens) {
    let p = palette(t);
    let g = ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), regular(10.5), p.agent));
    let (r, _) = ui.allocate_exact_size(g.size() + vec2(12.0, 4.0), Sense::hover());
    let fill = Color32::from_rgba_unmultiplied(p.agent.r(), p.agent.g(), p.agent.b(), 26);
    ui.painter().rect_filled(r, CornerRadius::same(8), fill);
    ui.painter().galley(r.min + vec2(6.0, 2.0), g, p.agent);
}

/// The mode's icon, drawn: one page, a speech bubble, or a terminal prompt.
fn mode_icon(pa: &egui::Painter, r: Rect, kind: Kind, c: Color32) {
    let s = egui::Stroke::new(1.6, c);
    match kind {
        Kind::None => {
            let page = Rect::from_center_size(r.center(), vec2(r.width() * 0.55, r.height() * 0.72));
            pa.rect_stroke(page, CornerRadius::same(2), s, egui::StrokeKind::Middle);
            for i in 0..3 {
                let y = page.min.y + page.height() * (0.3 + 0.2 * i as f32);
                pa.line_segment([pos2(page.min.x + 4.0, y), pos2(page.max.x - 4.0, y)], egui::Stroke::new(1.2, c));
            }
        }
        Kind::Cowork => {
            let b = Rect::from_center_size(r.center() - vec2(0.0, 2.0), vec2(r.width() * 0.78, r.height() * 0.55));
            pa.rect_stroke(b, CornerRadius::same(6), s, egui::StrokeKind::Middle);
            let tail = vec![pos2(b.min.x + 7.0, b.max.y), pos2(b.min.x + 5.0, b.max.y + 6.0), pos2(b.min.x + 13.0, b.max.y)];
            pa.add(egui::Shape::line(tail, s));
            for i in 0..3 {
                pa.circle_filled(pos2(b.center().x + (i as f32 - 1.0) * 6.0, b.center().y), 1.6, c);
            }
        }
        Kind::Code => {
            let b = Rect::from_center_size(r.center(), vec2(r.width() * 0.8, r.height() * 0.62));
            pa.rect_stroke(b, CornerRadius::same(3), s, egui::StrokeKind::Middle);
            let x = b.min.x + 6.0;
            let y = b.center().y;
            pa.add(egui::Shape::line(vec![pos2(x, y - 4.5), pos2(x + 5.0, y), pos2(x, y + 4.5)], s));
            pa.line_segment([pos2(x + 8.0, y + 5.0), pos2(x + 15.0, y + 5.0)], s);
        }
    }
}

/// The large mode button: the mode's icon with the lamp's dot, and its name.
fn mode_button(ui: &mut Ui, c: &Context, lamp: Lamp, tip: &str, t: &Tokens) -> Response {
    let label = match c.kind {
        Kind::None => "No\ncontext",
        Kind::Cowork => "Claude\nCowork",
        Kind::Code => "Claude\nCode",
    };
    big(ui, &format!("{label} ▾"), None, false, true, tip, |pa, rect, col| {
        mode_icon(pa, rect, c.kind, col);
        let d = pos2(rect.max.x - 3.0, rect.max.y - 4.0);
        pa.circle_filled(d, 5.5, t.ribbon);
        pa.circle_filled(d, 4.2, lamp.color(t));
    })
}

/// The lamp: a dot and Ready / Waiting / Not ready.
fn lamp_row(ui: &mut Ui, lamp: Lamp, tip: &str, extra: Option<&str>, t: &Tokens) {
    let text = match extra {
        Some(x) => format!("{} · {}", lamp.label(), x),
        None => lamp.label().to_string(),
    };
    let tw = ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.clone(), regular(11.5), t.text).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2(26.0 + tw, 22.0), Sense::hover());
    ui.painter().circle_filled(pos2(r.min.x + 12.0, r.center().y), 5.0, lamp.color(t));
    ui.painter().text(
        pos2(r.min.x + 24.0, r.center().y),
        Align2::LEFT_CENTER,
        text,
        regular(11.5),
        if lamp == Lamp::NotReady { t.text_dim } else { t.text },
    );
    resp.on_hover_text(tip);
}

/// A two-way switch: My session | Project folder.
fn toggle(ui: &mut Ui, on: CodeWay, t: &Tokens) -> Option<CodeWay> {
    let mut out = None;
    let font = regular(11.0);
    let parts = [CodeWay::Session, CodeWay::Folder];
    let ws: Vec<f32> =
        parts.iter().map(|w| ui.ctx().fonts_mut(|f| f.layout_no_wrap(w.label().to_string(), font.clone(), t.text).size().x) + 14.0).collect();
    let (r, _) = ui.allocate_exact_size(vec2(ws.iter().sum::<f32>() + 4.0, 22.0), Sense::hover());
    let frame = r.shrink2(vec2(0.0, 1.5));
    ui.painter().rect_stroke(frame, CornerRadius::same(5), egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let mut x = frame.min.x + 2.0;
    for (w, way) in ws.iter().zip(parts) {
        let seg = Rect::from_min_size(pos2(x, frame.min.y + 2.0), vec2(*w, frame.height() - 4.0));
        let resp = ui.interact(seg, ui.id().with(("code-way", way.id())), Sense::click());
        let tip = match way {
            CodeWay::Session => "Questions go to your own Claude Code session through AItoCraft",
            CodeWay::Folder => "Claude runs in the background in a project folder: it sees that folder's CLAUDE.md, memory and files",
        };
        let resp = resp.on_hover_text(tip);
        if way == on {
            ui.painter().rect_filled(seg, CornerRadius::same(4), t.checked);
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, CornerRadius::same(4), t.hover);
        }
        ui.painter().text(seg.center(), Align2::CENTER_CENTER, way.label(), font.clone(), t.text);
        if resp.clicked() && way != on {
            out = Some(way);
        }
        x += w;
    }
    out
}

/// Get ready ▾: the main part runs the next step not done; the arrow lists them all.
fn get_ready(st: &mut AgentUi, app: &WordApp, ui: &mut Ui, steps: &[Step], t: &Tokens, acts: &mut Vec<Act>) {
    let next = steps.iter().find(|s| !s.done).or(steps.last());
    let Some(next) = next else { return };
    let tip = if steps.iter().all(|s| s.done) { format!("Done. Again: {}", next.label) } else { format!("Next: {} — {}", next.label, next.tip) };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let main = small(ui, "chevronDoubleRight", "Get ready", true, &tip);
        let (ar, arrow) = ui.allocate_exact_size(vec2(16.0, 22.0), Sense::click());
        if arrow.hovered() {
            ui.painter().rect_filled(ar, CornerRadius::same(4), t.hover);
        }
        wordcraft_ui_egui::icons::paint(ui.painter(), Rect::from_center_size(ar.center(), vec2(12.0, 12.0)), "dropdown", t.icon, t.accent);
        let arrow = arrow.on_hover_text("All the steps");
        if main.clicked() {
            run_step(st, app, ui, next.id, acts);
        }
        scripted(ui, "ready", &arrow);
        egui::Popup::menu(&arrow).show(|ui| {
            ui.set_min_width(250.0);
            for s in steps {
                let mark = if s.done { "✔" } else { "   " };
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    mark,
                    0.0,
                    egui::TextFormat { font_id: regular(12.0), color: if s.done { Lamp::Ready.color(t) } else { t.text }, ..Default::default() },
                );
                job.append(&format!("  {}", s.label), 0.0, egui::TextFormat { font_id: regular(12.0), color: t.text, ..Default::default() });
                if ui.add(egui::Button::new(job).frame(false)).on_hover_text(&s.tip).clicked() {
                    run_step(st, app, ui, s.id, acts);
                    ui.close();
                }
            }
        });
    });
}

fn context_group(st: &mut AgentUi, ui: &mut Ui, app: &mut WordApp, t: &Tokens, acts: &mut Vec<Act>) {
    poll_folder(st);
    if let Some(n) = take_note() {
        acts.push(Act::Status(n));
    }
    let c = context::of(&app.session);
    let (lamp, tip) = lamp(st, app, &c, acts);
    let steps = steps(st, app, &c, lamp == Lamp::Ready);

    if wants_menu("none") {
        egui::Popup::close_all(ui.ctx());
    }
    // The mode.
    let m = mode_button(ui, &c, lamp, &format!("What the agent knows: {}. {tip}", c.describe()), t);
    if let Ok(mut g) = MODE_BUTTON.lock() {
        *g = Some(m.rect);
    }
    scripted(ui, "context", &m);
    egui::Popup::menu(&m).show(|ui| {
        ui.set_min_width(330.0);
        for k in Kind::ALL {
            let on = c.kind == k;
            let resp = ui
                .scope(|ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .corner_radius(4)
                        .fill(if on { t.checked } else { Color32::TRANSPARENT })
                        .show(ui, |ui| {
                            ui.set_min_width(314.0);
                            ui.horizontal(|ui| {
                                let (ir, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                                mode_icon(ui.painter(), ir, k, t.icon);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
                                    ui.label(egui::RichText::new(k.label()).font(wordcraft_ui_egui::theme::semibold(12.5)).color(t.text));
                                    ui.label(egui::RichText::new(k.line()).size(11.0).color(t.text_dim));
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                                        for x in k.chips() {
                                            chip(ui, x, t);
                                        }
                                    });
                                });
                            });
                        })
                        .response
                })
                .inner;
            let resp = ui.interact(resp.rect, ui.id().with(("context-kind", k.id())), Sense::click());
            if resp.hovered() && !on {
                ui.painter().rect_stroke(resp.rect, CornerRadius::same(4), egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            }
            if resp.clicked() {
                if !on {
                    acts.push(Act::Run("agent.setContext", json!({"kind": k.id()})));
                }
                ui.close();
            }
        }
    });
    // Right-click: the background session and Details. Its own popup id: egui's context menu
    // otherwise shares the mode menu's id and closes it on a left click.
    egui::Popup::context_menu(&m).id(m.id.with("agent_context_more")).show(|ui| more_menu(ui, app, t, acts));

    // What goes with it.
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
        match c.kind {
            Kind::None => {
                let mut web = c.web;
                if ui
                    .checkbox(&mut web, egui::RichText::new("Web").size(11.5))
                    .on_hover_text("The background client may also search and fetch the web (WebSearch, WebFetch)")
                    .changed()
                {
                    acts.push(Act::Run("agent.setContext", json!({"web": web})));
                }
                lamp_row(ui, lamp, &tip, None, t);
                if steps.iter().any(|s| !s.done) {
                    get_ready(st, app, ui, &steps, t, acts);
                }
            }
            Kind::Cowork => {
                lamp_row(ui, lamp, &tip, None, t);
                get_ready(st, app, ui, &steps, t, acts);
            }
            Kind::Code => {
                if let Some(w) = toggle(ui, c.code, t) {
                    acts.push(Act::Run("agent.setContext", json!({"kind": "code", "code": w.id()})));
                }
                lamp_row(ui, lamp, &tip, None, t);
                ui.horizontal(|ui| {
                    get_ready(st, app, ui, &steps, t, acts);
                    if c.code == CodeWay::Folder
                        && let Some(f) = c.project_folder()
                    {
                        let shown = home_short(&f);
                        ui.label(egui::RichText::new(clip(&shown, 34)).size(11.0).color(t.text_dim))
                            .on_hover_text(format!("{}\nClaude runs here: it sees this folder's CLAUDE.md, memory and files.", f.display()));
                    }
                });
            }
        }
    });
}

/// `~/projects/x` for a folder under the home folder.
fn home_short(p: &std::path::Path) -> String {
    let full = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && full.starts_with(&h) => format!("~{}", full.get(h.len()..).unwrap_or("")),
        _ => full,
    }
}

fn copy_item(ui: &mut Ui, acts: &mut Vec<Act>, label: &str, text: String) {
    if ui.button(label).on_hover_text(&text).clicked() {
        ui.ctx().copy_text(text);
        acts.push(Act::Status(format!("Copied: {label}")));
        ui.close();
    }
}

/// The mode button's right-click menu: the background session, and the connection's details
/// (with the ways in that don't need AItoCraft).
fn more_menu(ui: &mut Ui, app: &WordApp, t: &Tokens, acts: &mut Vec<Act>) {
    ui.set_min_width(300.0);
    let title = wordcraft_agent::document_title(&app.session);
    let st = runner::status(&app.session);
    if ui.button("Open the session in Claude Desktop").on_hover_text("Claude Desktop ▸ Code: the session named “WordCraft · <file>”").clicked()
    {
        let _ = std::process::Command::new("open").args(["-a", "Claude"]).spawn();
        let at = st.as_ref().map(|s| s.cwd.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "agent-sessions".into());
        acts.push(Act::Status(format!("In Claude Desktop: Code ▸ {at} ▸ “WordCraft · {title}”")));
        ui.close();
    }
    if ui.button("New session").on_hover_text("A fresh background session for this document, started from a summary of its history").clicked() {
        acts.push(Act::Run("agent.agentReset", json!({})));
        ui.close();
    }
    if ui.button("Stop the background client").clicked() {
        acts.push(Act::Run("agent.agentStop", json!({})));
        ui.close();
    }
    ui.separator();
    ui.menu_button("Details", |ui| details(ui, app, t, acts));
}

fn details(ui: &mut Ui, app: &WordApp, t: &Tokens, acts: &mut Vec<Act>) {
    ui.set_min_width(340.0);
    let url = crate::server_url().unwrap_or("http://127.0.0.1:7982/mcp").to_string();
    let base = url.trim_end_matches("/mcp").to_string();
    let entry = crate::stdio_entry();
    let exe = entry.as_ref().and_then(|e| e.get("command")).and_then(|c| c.as_str()).unwrap_or("wordcraft").to_string();
    let doc_id = wordcraft_agent::document_id(&app.session);
    let small_text = |ui: &mut Ui, s: String| {
        ui.label(egui::RichText::new(s).small().color(t.text_dim));
    };
    let c = context::of(&app.session);
    small_text(ui, format!("Document: {doc_id}"));
    small_text(ui, format!("Context: {} (connection mode: {})", c.describe(), presence::mode().id()));
    small_text(ui, format!("Server: {url}"));
    small_text(ui, format!("Events: {base}/events?document={doc_id}"));
    for a in presence::listeners_of(&doc_id) {
        small_text(ui, format!("Attached: {} (seen {})", a.label, ago(a.last_seen)));
    }
    let l = presence::listeners(runner::running(&app.session));
    if l.any() {
        small_text(ui, format!("Listening: {}", l.describe()));
    }
    let last = wordcraft_agent::last_agent_call_ms();
    if last > 0 {
        small_text(ui, format!("The agent's last tool call: {}", ago(last)));
    }
    match aitocraft::binary() {
        Some(b) => small_text(ui, format!("AItoCraft: {}", b.display())),
        None => small_text(ui, "AItoCraft: not found (PATH, ~/aitocraft/target/release)".into()),
    }
    if let Some(f) = wordcraft_agent::registry::file() {
        small_text(ui, format!("Registry: {}", f.display()));
    }
    if let Some(st) = runner::status(&app.session) {
        small_text(ui, format!("Background client: {} · {} turn(s) · ${:.2}", if st.running { "running" } else { "stopped" }, st.turns, st.cost_usd));
        if !st.cwd.is_empty() {
            small_text(ui, format!("Folder: {}", st.cwd));
        }
        if !st.last_error.is_empty() {
            small_text(ui, format!("Last error: {}", st.last_error));
        }
        if !st.session_id.is_empty() {
            copy_item(ui, acts, "Copy: continue the session by hand", runner::resume_command(&st));
        }
    }
    ui.separator();
    ui.label(egui::RichText::new("Without AItoCraft (this app only)").small().strong());
    copy_item(ui, acts, "Claude Code: add WordCraft over HTTP", format!("claude mcp add --transport http wordcraft {url}"));
    copy_item(ui, acts, "Monitor command", format!("curl -sN '{base}/events?document={doc_id}'"));
    copy_item(
        ui,
        acts,
        "Claude Code channel: add WordCraft (stdio)",
        format!("claude mcp add {} -- '{}' --agent-stdio", wordcraft_agent::channel::SERVER_NAME, exe.replace('\'', "'\\''")),
    );
    let state = cowork::state();
    let configured = match (&entry, cowork::config_path()) {
        (Some(e), Some(path)) => cowork::configured(&path, e),
        _ => Configured::NoClaude,
    };
    let note = match (&state, &configured) {
        (State::Waiting, _) => " (quit Claude: Cmd+Q)",
        (State::Done { .. }, _) => " (open Claude)",
        (State::Failed(_), _) => " (failed)",
        (_, Configured::Yes) => " (connected)",
        _ => "",
    };
    let can = entry.is_some() && state != State::Waiting;
    if ui
        .add_enabled(can, egui::Button::new(format!("Claude Desktop: add WordCraft (--agent-stdio){note}")))
        .on_hover_text("The fallback without AItoCraft: adds `wordcraft --agent-stdio` to Claude Desktop. If Claude is open, WordCraft waits for you to quit it (Cmd+Q), adds itself, and you open Claude again.")
        .clicked()
        && let Some(e) = entry.clone()
    {
        cowork::connect(e);
        let msg = match cowork::state() {
            State::Waiting => "Quit Claude Desktop with Cmd+Q: WordCraft adds itself as soon as it has quit. Then open Claude again.".to_string(),
            State::Done { .. } => "Added to Claude Desktop: open Claude.".to_string(),
            State::Failed(e) => e,
            State::Idle => String::new(),
        };
        acts.push(Act::Status(msg));
        ui.close();
    }
}
