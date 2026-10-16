//! The Agent tab on the ribbon: what isn't about one passage. Groups (the same in GridCraft and
//! DeckCraft):
//!
//! - **Agent**: opens the pane, and says what is waiting (answers ready, questions with the agent).
//! - **Client data**: mark or unmark the selection, the marks in the document, and for a template
//!   the client it is shown for.
//! - **Connection**: is anyone listening (and a Check that asks "is anyone there?"), how the agent
//!   comes in (the mode), and a Copy menu with exactly what that mode needs.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Ui, pos2, vec2};
use serde_json::json;
use wordcraft_agent::aitocraft;
use wordcraft_agent::anchor;
use wordcraft_agent::cowork::{self, Configured, State};
use wordcraft_agent::layer::QuestionStatus;
use wordcraft_agent::mask;
use wordcraft_agent::presence::{self, Mode};
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
    let ready = l.questions.iter().filter(|q| q.status == QuestionStatus::Answered && !q.seen).count();
    let working = l.questions.iter().filter(|q| q.status == QuestionStatus::Working).count();
    let waiting = l.questions.iter().filter(|q| q.status == QuestionStatus::Pending).count();
    if ready > 0 {
        (format!("{ready} ready"), p.answered)
    } else if working > 0 {
        (format!("{working} in work"), p.working)
    } else if waiting > 0 {
        (format!("{waiting} waiting"), p.pending)
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

    group(ui, "Agent", None, app, |ui, _app| {
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
                acts.push(Act::Open(Tab::Questions, None));
            }
        }
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
                let name_of = |id: &str| clients.iter().find(|c| c["id"] == id).and_then(|c| c["name"].as_str()).unwrap_or(id).to_string();
                let shown = current.as_deref().map_or_else(|| "Template".to_string(), name_of);
                let pick = small(ui, "mergeField", &format!("Show: {}", clip(&shown, 22)), true, "Show the template filled in for a client (read only). Your agent keeps working on the template and never sees the client's data.");
                egui::Popup::menu(&pick).show(|ui| {
                    if ui.selectable_label(current.is_none(), "The template (placeholders)").clicked() {
                        acts.push(Act::Run("agent.render", json!({"client": null})));
                        ui.close();
                    }
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

    group(ui, "Connection", None, app, |ui, app| connection(ui, app, &t, &mut acts));
    st.acts.extend(acts);
}

static MENU_REQUEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Opens one of the Connection menus (`mode`, `copy`) on the next frame (for scripts and snapshots).
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

/// The status indicator: a filled dot, a half dot, a ring or a cross (drawn: the interface fonts
/// lack the glyphs).
#[derive(Clone, Copy)]
enum Dot {
    Full,
    Half,
    Ring,
    Cross,
}

fn dot(p: &egui::Painter, c: egui::Pos2, kind: Dot, color: Color32) {
    let r = 5.5;
    match kind {
        Dot::Full => {
            p.circle_filled(c, r, color);
        }
        Dot::Half => {
            p.circle_stroke(c, r, egui::Stroke::new(1.5, color));
            let pts: Vec<egui::Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.5 + i as f32 / 16.0);
                    c + vec2(a.cos() * r, a.sin() * r)
                })
                .collect();
            p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
        }
        Dot::Ring => {
            p.circle_stroke(c, r, egui::Stroke::new(1.5, color));
        }
        Dot::Cross => {
            let d = r * 0.8;
            p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], egui::Stroke::new(2.0, color));
            p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], egui::Stroke::new(2.0, color));
        }
    }
}

/// The status of this document's connection: indicator, line, sub-line, tooltip.
fn status_view(app: &WordApp, t: &Tokens) -> (Dot, Color32, String, String, String) {
    let p = palette(t);
    let red = Color32::from_rgb(0xC4, 0x2B, 0x1C);
    let now = wordcraft_agent::now_ms();
    let ping = wordcraft_agent::ping(&app.session);
    let bg = runner::running(&app.session);
    let l = presence::listeners(bg);
    let attached = presence::listeners_of(&wordcraft_agent::document_id(&app.session));
    let last = wordcraft_agent::last_agent_call_ms();
    let mut tip = String::new();
    for a in &attached {
        tip.push_str(&format!("Attached: {} (seen {}). ", a.label, ago(a.last_seen)));
    }
    if l.any() {
        tip.push_str(&format!("Listening: {}. ", l.describe()));
    }
    tip.push_str(&if last > 0 { format!("The agent's last tool call: {}.", ago(last)) } else { "No tool call from an agent yet.".into() });
    let mode = presence::mode();
    let (d, c, line, sub) = match presence::status(ping.as_ref(), &attached, &l, now) {
        presence::Status::Answering { checked_at, secs, by } => {
            let line = if by.is_empty() { "Agent answering".to_string() } else { format!("Agent answering: {by}") };
            (Dot::Full, p.answered, line, format!("checked {} · answered in {secs} s", ago(checked_at)))
        }
        presence::Status::Checking => {
            let n = ping.as_ref().map_or(0, |x| now.saturating_sub(x.asked_at) / 1000);
            (Dot::Half, p.pending, "Checking…".to_string(), format!("asked {n} s ago, waiting for an answer"))
        }
        presence::Status::NoAnswer => {
            let sub =
                if l.any() || !attached.is_empty() { "a listener is connected, but nobody answered" } else { "nobody answered within a minute" };
            (Dot::Cross, red, "No answer to check".to_string(), sub.to_string())
        }
        presence::Status::Attached { label, last_seen, others } => {
            let more = if others > 0 { format!(" +{others}") } else { String::new() };
            (Dot::Full, p.answered, format!("Listening: {label}{more}"), format!("seen {} · not checked", ago(last_seen)))
        }
        presence::Status::Listener => {
            (Dot::Half, p.pending, "Listener connected".to_string(), format!("not attached, not checked · {}", l.describe()))
        }
        presence::Status::Nobody => {
            let sub = match mode {
                Mode::Connect => "questions queue here · Copy phrase for Claude",
                Mode::Channel => "questions queue here · start Claude Code (Copy)",
                Mode::Background => "the agent starts with the first question",
                Mode::Off => "",
            };
            (Dot::Ring, t.text_disabled, "Nobody is listening for this document".to_string(), sub.to_string())
        }
        presence::Status::Off => (Dot::Ring, t.text_disabled, "Agent connection off".to_string(), "choose a mode to let an agent in".to_string()),
    };
    // The background agent's own trouble shows here too.
    let sub = match runner::status(&app.session) {
        Some(st) if mode == Mode::Background && !st.last_error.is_empty() && !st.running => format!("agent stopped: {}", clip(&st.last_error, 40)),
        _ => sub,
    };
    (d, c, line, sub, tip)
}

fn copy_item(ui: &mut Ui, acts: &mut Vec<Act>, label: &str, text: String) {
    if ui.button(label).on_hover_text(&text).clicked() {
        ui.ctx().copy_text(text);
        acts.push(Act::Status(format!("Copied: {label}")));
        ui.close();
    }
}

/// "Install AItoCraft in Claude Desktop": runs the hub's installer when its binary is found.
fn install_item(ui: &mut Ui, acts: &mut Vec<Act>) {
    let bin = aitocraft::binary();
    let state = aitocraft::install_state();
    let note = match &state {
        aitocraft::Install::Running => " (installing…)",
        aitocraft::Install::Done(_) => " (done)",
        aitocraft::Install::Failed(_) => " (failed)",
        aitocraft::Install::Idle => "",
    };
    let tip = match (&state, &bin) {
        (aitocraft::Install::Done(m) | aitocraft::Install::Failed(m), _) => m.clone(),
        (_, Some(b)) => format!(
            "Runs {} --install-desktop: AItoCraft becomes the one MCP server Claude Desktop and Cowork use for every Craft document",
            b.display()
        ),
        (_, None) => "AItoCraft isn't installed yet: build it (~/aitocraft) or put `aitocraft` on PATH".into(),
    };
    let can = bin.is_some() && state != aitocraft::Install::Running;
    if ui
        .add_enabled(can, egui::Button::new(format!("Install AItoCraft in Claude Desktop{note}")))
        .on_hover_text(tip)
        .on_disabled_hover_text("AItoCraft isn't installed yet: build it (~/aitocraft) or put `aitocraft` on PATH")
        .clicked()
    {
        aitocraft::install_desktop();
        acts.push(Act::Status("Installing AItoCraft in Claude Desktop… then quit Claude (Cmd+Q) and open it again".into()));
        ui.close();
    }
}

fn connection(ui: &mut Ui, app: &mut WordApp, t: &Tokens, acts: &mut Vec<Act>) {
    let mode = presence::mode();
    let url = crate::server_url().unwrap_or("http://127.0.0.1:7982/mcp").to_string();
    let base = url.trim_end_matches("/mcp").to_string();
    let entry = crate::stdio_entry();
    let exe = entry.as_ref().and_then(|e| e.get("command")).and_then(|c| c.as_str()).unwrap_or("wordcraft").to_string();
    let title = wordcraft_agent::document_title(&app.session);
    let doc_id = wordcraft_agent::document_id(&app.session);
    let (d, color, line, sub, tip) = status_view(app, t);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 1.0);
        // The status of this document, big.
        let w = 300.0;
        let (r, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
        dot(ui.painter(), pos2(r.min.x + 8.0, r.min.y + 10.0), d, color);
        ui.painter().text(pos2(r.min.x + 20.0, r.min.y + 10.0), Align2::LEFT_CENTER, clip(&line, 44), wordcraft_ui_egui::theme::semibold(12.5), t.text);
        ui.painter().text(pos2(r.min.x + 20.0, r.min.y + 27.0), Align2::LEFT_CENTER, clip(&sub, 54), regular(10.5), t.text_dim);
        resp.on_hover_text(&tip);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
            let can_check = mode != Mode::Off;
            if small(ui, "check", "Check", can_check, "Ask \"is anyone there?\": a connection check this document's listeners answer at once with \"here\" (never shown as a question)").clicked() {
                acts.push(Act::Run("agent.ping", json!({})));
            }
            // The mode.
            let m = small(ui, "dropdown", &format!("Mode: {}", clip(mode.short(), 24)), true, "How the agent comes in");
            let close = wants_menu("none");
            if wants_menu("mode") {
                egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&m));
            }
            egui::Popup::menu(&m).show(|ui| {
                ui.set_min_width(440.0);
                for x in Mode::ALL {
                    let mut job = egui::text::LayoutJob::default();
                    job.append(x.label(), 0.0, egui::TextFormat { font_id: wordcraft_ui_egui::theme::semibold(12.0), color: t.text, ..Default::default() });
                    job.append(&format!("\n{}", x.explain()), 0.0, egui::TextFormat { font_id: regular(10.5), color: t.text_dim, ..Default::default() });
                    if ui.selectable_label(mode == x, job).clicked() {
                        acts.push(Act::Run("agent.agentMode", json!({"mode": x.id()})));
                        ui.close();
                    }
                }
            });
            // What to copy for this mode.
            let c = small(ui, "copy", "Copy", true, "Exactly what this mode needs, one thing per item");
            if close {
                egui::Popup::close_id(ui.ctx(), egui::Popup::default_response_id(&m));
                egui::Popup::close_id(ui.ctx(), egui::Popup::default_response_id(&c));
            }
            if wants_menu("copy") {
                egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&c));
            }
            egui::Popup::menu(&c).show(|ui| {
                ui.set_min_width(290.0);
                let hub = aitocraft::binary();
                match mode {
                    Mode::Connect => {
                        copy_item(ui, acts, "Copy phrase for Claude", aitocraft::phrase(&title));
                        install_item(ui, acts);
                        copy_item(ui, acts, "Claude Code: add AItoCraft", aitocraft::claude_code_command(hub.as_deref()));
                    }
                    Mode::Channel => {
                        copy_item(ui, acts, "Copy phrase for Claude", aitocraft::phrase(&title));
                        copy_item(ui, acts, "Claude Code: add AItoCraft", aitocraft::claude_code_command(hub.as_deref()));
                        copy_item(ui, acts, "Start Claude Code with the channel", aitocraft::CHANNEL_COMMAND.to_string());
                    }
                    Mode::Background => {
                        let st = runner::status(&app.session);
                        if ui.button("Open session in Claude Desktop").on_hover_text("Claude Desktop ▸ Code: the session named “WordCraft · <file>”").clicked() {
                            let _ = std::process::Command::new("open").args(["-a", "Claude"]).spawn();
                            let at = st.as_ref().map(|s| s.cwd.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "agent-sessions".into());
                            acts.push(Act::Status(format!("In Claude Desktop: Code ▸ {at} ▸ “WordCraft · {title}”")));
                            ui.close();
                        }
                        if ui.button("New session").on_hover_text("A fresh session for this document, started from a summary of its history").clicked() {
                            acts.push(Act::Run("agent.agentReset", json!({})));
                            ui.close();
                        }
                        let mut folder = runner::config().document_folder;
                        if ui
                            .checkbox(&mut folder, "Run in the document's folder")
                            .on_hover_text("The agent's project becomes the document's folder: that folder's CLAUDE.md, memory and settings become its context, and its sessions sit with the folder's in Claude Desktop. Off: a neutral, empty folder (ArtCraft/agent-sessions).")
                            .changed()
                        {
                            runner::set_document_folder(folder);
                            acts.push(Act::Run("agent.agentStop", json!({})));
                        }
                    }
                    Mode::Off => {
                        ui.label(egui::RichText::new("Nothing to copy: the connection is off").color(t.text_disabled));
                    }
                }
                ui.separator();
                ui.menu_button("Details", |ui| {
                    ui.set_min_width(340.0);
                    let small_text = |ui: &mut Ui, s: String| {
                        ui.label(egui::RichText::new(s).small().color(t.text_dim));
                    };
                    small_text(ui, format!("Document: {doc_id}"));
                    small_text(ui, format!("Server: {url}"));
                    small_text(ui, format!("Events: {base}/events?document={doc_id}"));
                    small_text(ui, format!("Mode: {} ({})", mode.label(), mode.id()));
                    small_text(ui, tip.clone());
                    match aitocraft::binary() {
                        Some(b) => small_text(ui, format!("AItoCraft: {}", b.display())),
                        None => small_text(ui, "AItoCraft: not found (PATH, ~/aitocraft/target/release)".into()),
                    }
                    if let Some(f) = wordcraft_agent::registry::file() {
                        small_text(ui, format!("Registry: {}", f.display()));
                    }
                    if let Some(st) = runner::status(&app.session) {
                        small_text(ui, format!("Background agent: {} · {} turn(s) · ${:.2}", if st.running { "running" } else { "stopped" }, st.turns, st.cost_usd));
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
                    copy_item(ui, acts, "Claude Code channel: add WordCraft (stdio)", format!("claude mcp add {} -- '{}' --agent-stdio", wordcraft_agent::channel::SERVER_NAME, exe.replace('\'', "'\\''")));
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
                });
            });
        });
    });
}
