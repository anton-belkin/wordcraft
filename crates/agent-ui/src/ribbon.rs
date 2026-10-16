//! The Agent tab on the ribbon: what isn't about one passage. Groups (the same in GridCraft and
//! DeckCraft):
//!
//! - **Agent**: opens the pane, and says what is waiting (answers ready, questions with the agent).
//! - **Client data**: mark or unmark the selection, the marks in the document, and for a template
//!   the client it is shown for.
//! - **Connection**: the server, whether an agent is talking to it, the Claude Code command, and
//!   Connect Cowork (adds WordCraft to Claude Desktop).

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Ui, pos2, vec2};
use serde_json::json;
use wordcraft_agent::anchor;
use wordcraft_agent::cowork::{self, Configured, State};
use wordcraft_agent::layer::QuestionStatus;
use wordcraft_agent::mask;
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

/// A line of status text with a coloured dot, as tall as a small button.
fn status_line(ui: &mut Ui, dot: Color32, text: &str, tip: &str) {
    let t = Tokens::get(ui.ctx());
    let tw = ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), regular(11.0), t.text).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2(18.0 + tw, 22.0), Sense::hover());
    ui.painter().circle_filled(pos2(r.min.x + 7.0, r.center().y), 4.0, dot);
    ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, text, regular(11.0), t.text_dim);
    resp.on_hover_text(tip);
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

    group(ui, "Connection", None, app, |ui, _app| {
        let url = crate::server_url();
        let last = wordcraft_agent::last_agent_call_ms();
        let now = wordcraft_agent::now_ms();
        let active = last > 0 && now.saturating_sub(last) < 120_000;
        let entry = crate::stdio_entry();
        let path = cowork::config_path();
        let configured = match (&entry, &path) {
            (Some(e), Some(p)) => cowork::configured(p, e),
            _ => Configured::NoClaude,
        };
        let state = cowork::state();
        let (cw_sub, cw_color) = match (&state, &configured) {
            (State::Waiting, _) => ("quit Claude (Cmd+Q)".to_string(), p.pending),
            (State::Done { .. }, _) => ("open Claude".to_string(), p.answered),
            (State::Failed(_), _) => ("failed".to_string(), Color32::from_rgb(0xC4, 0x2B, 0x1C)),
            (_, Configured::Yes) => ("connected".to_string(), p.answered),
            (_, Configured::Different) => ("other build".to_string(), p.pending),
            _ => ("not set up".to_string(), t.text_disabled),
        };
        let tip = match &state {
            State::Waiting => "Claude Desktop rewrites its settings when it quits, so WordCraft waits: quit Claude with Cmd+Q, WordCraft adds itself, then open Claude again.".to_string(),
            State::Done { backup } => format!(
                "Added to Claude Desktop{}. Open Claude (or quit it with Cmd+Q and open it again): the wordcraft tools appear in Cowork and chats.",
                backup.as_ref().map(|b| format!(" (old settings saved as {})", b.display())).unwrap_or_default()
            ),
            State::Failed(e) => e.clone(),
            State::Idle => "Add WordCraft to Claude Desktop, so Cowork and chats there can answer your questions. Other servers in its settings stay as they are.".to_string(),
        };
        let can = entry.is_some() && path.is_some() && state != State::Waiting;
        if big(ui, "Connect\nCowork", Some((&cw_sub, cw_color)), configured == Configured::Yes, can, &tip, |pa, rect, c| {
            wordcraft_ui_egui::icons::paint(pa, rect.shrink(1.0), "addins", c, p.agent)
        })
        .clicked()
            && let Some(e) = entry
        {
            cowork::connect(e);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
            match url {
                Some(u) => status_line(
                    ui,
                    p.answered,
                    &format!("Listening · {}", u.trim_start_matches("http://").trim_end_matches("/mcp")),
                    "The agent server (MCP over HTTP, this computer only)",
                ),
                None => status_line(ui, t.text_disabled, "Server off", "Start WordCraft without --no-agent"),
            }
            if active {
                status_line(ui, p.agent, &format!("Agent active · {}", ago(last)), "An agent called the layer in the last two minutes");
            } else if last > 0 {
                status_line(ui, t.text_disabled, &format!("Agent last seen {}", ago(last)), "");
            } else {
                status_line(ui, t.text_disabled, "No agent connected yet", "Connect Claude Code (Copy command) or Cowork");
            }
            let cmd = format!("claude mcp add --transport http wordcraft {}", url.unwrap_or("http://127.0.0.1:7982/mcp"));
            if small(ui, "copy", "Copy Claude Code command", url.is_some(), &cmd).clicked() {
                ui.ctx().copy_text(cmd);
                acts.push(Act::Status("Copied: paste it into a terminal to connect Claude Code".into()));
            }
        });
    });
    st.acts.extend(acts);
}
