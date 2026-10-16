//! The Agent pane: ask about the selection, all questions with their answers, the agent's notes,
//! client data and the template's client, the history, and how to connect an agent.

use egui::{Color32, RichText, Ui};
use serde_json::json;
use wordcraft_agent::anchor::{self, Span};
use wordcraft_agent::layer::{Actor, Layer, Link, Note, Question, QuestionStatus};
use wordcraft_agent::mask;
use wordcraft_doc::Path;
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::theme::{self, Tokens};

use crate::{Act, AgentUi, Palette, Tab, ago, clip, kind_label, palette, selection_span, span_of, span_params, span_text, status_color, status_text};

pub(crate) fn show(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let p = palette(&t);
    let max = (ui.available_width() / 2.0).max(340.0);
    let mut close = false;
    egui::Panel::right("agent_pane")
        .default_size(360.0)
        .min_size(300.0)
        .max_size(max)
        .resizable(true)
        .frame(egui::Frame::NONE.fill(t.panel).inner_margin(12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                crate::sparkle(ui, p.agent, 16.0);
                ui.label(RichText::new("Your agent").font(theme::semibold(15.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if wordcraft_ui_egui::widgets::icon_button(ui, "close", "Close").clicked() {
                        close = true;
                    }
                });
            });
            let l = wordcraft_agent::layer(&app.session);
            let ready = l.questions.iter().filter(|q| q.status == QuestionStatus::Answered && !q.seen).count();
            let open = l.questions.iter().filter(|q| q.status != QuestionStatus::Answered).count();
            ui.horizontal_wrapped(|ui| {
                let qlabel = match (ready, open) {
                    (0, 0) => "Questions".to_string(),
                    (0, n) => format!("Questions ({n})"),
                    (r, _) => format!("Questions ● {r}"),
                };
                for (tab, name) in [
                    (Tab::Ask, "Ask".to_string()),
                    (Tab::Questions, qlabel),
                    (Tab::Notes, format!("Notes ({})", l.notes.len())),
                    (Tab::Client, "Client data".into()),
                    (Tab::History, "History".into()),
                    (Tab::Connect, "Connect".into()),
                ] {
                    if ui.selectable_label(st.tab == tab, name).clicked() {
                        st.tab = tab;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match st.tab {
                Tab::Ask => ask_tab(st, app, ui, &l, &t, &p),
                Tab::Questions => questions_tab(st, app, ui, &l, &t, &p),
                Tab::Notes => notes_tab(st, app, ui, &l, &t, &p),
                Tab::Client => client_tab(st, app, ui, &l, &t, &p),
                Tab::History => history_tab(ui, &l, &t, &p),
                Tab::Connect => connect_tab(ui, &t),
            });
        });
    if close {
        st.open = false;
    }
}

fn card(ui: &mut Ui, t: &Tokens, stripe: Color32, highlight: bool, add: impl FnOnce(&mut Ui)) {
    let stroke = if highlight { egui::Stroke::new(1.5, stripe) } else { egui::Stroke::new(1.0, t.border) };
    egui::Frame::NONE.fill(t.input).corner_radius(6.0).inner_margin(egui::Margin { left: 12, right: 8, top: 8, bottom: 8 }).stroke(stroke).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            let top = ui.cursor().min;
            add(ui);
            let bottom = ui.min_rect().bottom();
            ui.painter().line_segment([egui::pos2(top.x - 7.0, top.y), egui::pos2(top.x - 7.0, bottom)], egui::Stroke::new(3.0, stripe));
        },
    );
    ui.add_space(6.0);
}

fn links(ui: &mut Ui, t: &Tokens, list: &[Link], acts: &mut Vec<Act>) {
    for k in list {
        let icon = if k.href.starts_with("para:") {
            "¶"
        } else if k.href.starts_with("note:") {
            "→"
        } else {
            "↗"
        };
        let r = ui.link(RichText::new(format!("{icon} {}", k.label)).color(t.accent_text)).on_hover_text(&k.href);
        if r.clicked() {
            acts.push(Act::Follow(k.href.clone()));
        }
        if !k.summary.is_empty() {
            ui.label(RichText::new(&k.summary).color(t.text_dim).small());
        }
    }
}

fn quote_link(app: &WordApp, ui: &mut Ui, t: &Tokens, sp: Option<Span>, fallback: &str, acts: &mut Vec<Act>) {
    match sp {
        Some(sp) => {
            let text = span_text(&app.session, &sp);
            let shown = if text.trim().is_empty() { format!("¶ {}", sp.para) } else { format!("“{}”", clip(text.trim(), 90)) };
            if ui.link(RichText::new(shown).italics().color(t.text_dim)).on_hover_text("Show in the document").clicked() {
                acts.push(Act::GoTo(sp));
            }
        }
        None => {
            ui.label(RichText::new(format!("“{}” (no longer in the text)", clip(fallback, 80))).italics().color(t.text_disabled));
        }
    }
}

fn question_card(app: &WordApp, ui: &mut Ui, paras: &[Path], l: &Layer, q: &Question, t: &Tokens, p: &Palette, acts: &mut Vec<Act>, highlight: bool) {
    let color = status_color(q, p);
    let (icon, status) = status_text(q);
    card(ui, t, color, highlight, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{icon} {status}")).color(color).font(theme::semibold(11.5)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(ago(q.asked_at)).color(t.text_disabled).small());
            });
        });
        let sp = span_of(&app.session, paras, l, &q.anchor);
        let fallback = l.anchor(&q.anchor).map(|a| a.quote.clone()).unwrap_or_default();
        quote_link(app, ui, t, sp, &fallback, acts);
        ui.label(RichText::new(&q.text).font(theme::semibold(13.0)));
        if let Some(a) = &q.answer {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                crate::sparkle(ui, p.agent, 11.0);
                ui.label(RichText::new("Your agent").color(p.agent).small().strong());
            });
            ui.label(&a.text);
            links(ui, t, &a.links, acts);
            if !q.seen && ui.small_button("Mark as read").clicked() {
                acts.push(Act::Run("agent.seen", json!({"id": q.id})));
            }
        }
    });
}

fn note_card(app: &WordApp, ui: &mut Ui, paras: &[Path], l: &Layer, n: &Note, t: &Tokens, p: &Palette, acts: &mut Vec<Act>, highlight: bool) {
    let (icon, kind) = kind_label(&n.kind);
    card(ui, t, p.note, highlight, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{icon} {kind}")).color(p.note).font(theme::semibold(11.5)));
            ui.label(RichText::new(if n.author == Actor::Agent { "by your agent" } else { "by a person" }).color(t.text_disabled).small());
        });
        let sp = span_of(&app.session, paras, l, &n.anchor);
        let fallback = l.anchor(&n.anchor).map(|a| a.quote.clone()).unwrap_or_default();
        quote_link(app, ui, t, sp.clone(), &fallback, acts);
        if !n.title.is_empty() {
            ui.label(RichText::new(&n.title).font(theme::semibold(13.0)));
        }
        ui.label(&n.text);
        links(ui, t, &n.links, acts);
        ui.horizontal(|ui| {
            if let Some(sp) = sp
                && ui.small_button("Ask about this").clicked()
            {
                acts.push(Act::AskAbout(sp));
            }
            if ui.small_button("Remove").clicked() {
                acts.push(Act::Run("agent.removeNote", json!({"id": n.id})));
            }
        });
    });
}

fn ask_tab(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui, l: &Layer, t: &Tokens, p: &Palette) {
    let mut acts = vec![];
    let preview = wordcraft_agent::render_state(&app.session).is_some();
    let sp = st
        .ask_span
        .clone()
        .filter(|sp| anchor::para_text(&app.session.doc, &sp.path).is_some_and(|x| x.len() >= sp.end))
        .or_else(|| selection_span(&app.session));
    ui.label(RichText::new("Ask about").color(t.text_dim).small());
    match &sp {
        Some(sp) => {
            let text = span_text(&app.session, sp);
            let shown = if text.trim().is_empty() { format!("paragraph {}", sp.para) } else { format!("“{}”", clip(text.trim(), 160)) };
            ui.label(RichText::new(shown).italics());
        }
        None => {
            ui.label(RichText::new("Select a passage in the document").color(t.text_disabled));
        }
    }
    ui.add_space(4.0);
    let edit = egui::TextEdit::multiline(&mut st.draft)
        .hint_text("What would you like to know? Your agent answers in the background; keep reading.")
        .desired_rows(3)
        .desired_width(f32::INFINITY);
    let r = ui.add(edit);
    if std::mem::take(&mut st.focus_draft) {
        r.request_focus();
    }
    let send_key = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.horizontal(|ui| {
        let can = sp.is_some() && !st.draft.trim().is_empty();
        if (ui.add_enabled(can, egui::Button::new(RichText::new("Ask your agent").strong().color(Color32::WHITE)).fill(p.agent)).clicked()
            || (send_key && can))
            && let Some(sp) = &sp
        {
            let mut params = span_params(&app.session, sp);
            params["text"] = json!(st.draft.trim());
            acts.push(Act::Run("agent.ask", params));
            st.draft.clear();
            st.ask_span = None;
        }
        ui.label(RichText::new(if cfg!(target_os = "macos") { "Cmd+Enter" } else { "Ctrl+Enter" }).color(t.text_disabled).small());
    });
    if preview {
        ui.label(RichText::new("You are looking at the template rendered for a client. Your agent sees the template.").color(t.text_dim).small());
    }
    ui.add_space(8.0);
    // What is already on this paragraph.
    if let Some(sp) = &sp {
        let paras = anchor::paragraphs(&app.session.doc);
        let here: Vec<&Question> =
            l.questions.iter().filter(|q| span_of(&app.session, &paras, l, &q.anchor).is_some_and(|x| x.path == sp.path)).collect();
        let notes: Vec<&Note> = l.notes.iter().filter(|n| span_of(&app.session, &paras, l, &n.anchor).is_some_and(|x| x.path == sp.path)).collect();
        if !here.is_empty() || !notes.is_empty() {
            ui.label(RichText::new("On this paragraph").color(t.text_dim).small());
        }
        for n in notes {
            note_card(app, ui, &paras, l, n, t, p, &mut acts, false);
        }
        for q in here {
            question_card(app, ui, &paras, l, q, t, p, &mut acts, false);
        }
    }
    st.acts.extend(acts);
}

fn questions_tab(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui, l: &Layer, t: &Tokens, p: &Palette) {
    let mut acts = vec![];
    if l.questions.is_empty() {
        ui.label(RichText::new("No questions yet. Select a passage and choose Ask the Agent (right-click), or use the Ask tab.").color(t.text_dim));
    }
    let paras = anchor::paragraphs(&app.session.doc);
    // Answers ready first, then those still with the agent, then the rest; newest first in each.
    let rank = |q: &Question| match q.status {
        QuestionStatus::Answered if !q.seen => 0,
        QuestionStatus::Working => 1,
        QuestionStatus::Pending => 2,
        QuestionStatus::Answered => 3,
    };
    let mut list: Vec<&Question> = l.questions.iter().collect();
    list.sort_by(|a, b| rank(a).cmp(&rank(b)).then(b.asked_at.cmp(&a.asked_at)));
    for q in list {
        let hl = st.highlight.as_deref() == Some(q.id.as_str());
        question_card(app, ui, &paras, l, q, t, p, &mut acts, hl);
    }
    st.acts.extend(acts);
}

fn notes_tab(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui, l: &Layer, t: &Tokens, p: &Palette) {
    let mut acts = vec![];
    if l.notes.is_empty() {
        ui.label(RichText::new("Your agent's explanations, prepared answers and sources show here and in the margin.").color(t.text_dim));
    }
    let paras = anchor::paragraphs(&app.session.doc);
    let mut list: Vec<(Option<Span>, &Note)> = l.notes.iter().map(|n| (span_of(&app.session, &paras, l, &n.anchor), n)).collect();
    list.sort_by_key(|(sp, _)| sp.as_ref().map_or((usize::MAX, 0), |s| (s.para, s.start)));
    for (_, n) in list {
        let hl = st.highlight.as_deref() == Some(n.id.as_str());
        note_card(app, ui, &paras, l, n, t, p, &mut acts, hl);
    }
    st.acts.extend(acts);
}

fn client_tab(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui, l: &Layer, t: &Tokens, p: &Palette) {
    let mut acts = vec![];
    let render = wordcraft_agent::render_state(&app.session).map(|r| r.client.clone());
    let template = wordcraft_agent::template::is_template(wordcraft_agent::agent_doc(&app.session));
    if template || render.is_some() {
        ui.label(RichText::new("Template").font(theme::semibold(13.0)));
        ui.label(
            RichText::new("Show the document filled in for a client. Your agent keeps working on the template and never sees the client's data.")
                .color(t.text_dim)
                .small(),
        );
        let clients = wordcraft_agent::crm::clients();
        let name_of = |id: &str| clients.iter().find(|c| c["id"] == id).and_then(|c| c["name"].as_str()).unwrap_or(id).to_string();
        let shown = render.as_deref().map_or_else(|| "The template (placeholders)".to_string(), name_of);
        egui::ComboBox::from_id_salt("agent_client").selected_text(shown).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
            if ui.selectable_label(render.is_none(), "The template (placeholders)").clicked() {
                acts.push(Act::Run("agent.render", json!({"client": null})));
            }
            for c in &clients {
                let id = c["id"].as_str().unwrap_or("");
                let name = c["name"].as_str().unwrap_or(id);
                if ui.selectable_label(render.as_deref() == Some(id), format!("{name}  ·  {id}")).clicked() {
                    acts.push(Act::Run("agent.render", json!({"client": id})));
                }
            }
        });
        ui.add_space(10.0);
    } else if ui.button("Open the demo template (engagement letter)").clicked() {
        acts.push(Act::Run("agent.openDemo", json!({})));
    }
    ui.label(RichText::new("Client data").font(theme::semibold(13.0)));
    ui.label(
        RichText::new("Text marked as client data reads as a {{placeholder}} in everything your agent sees, and the agent can't change it. Only you can lift the mark.")
            .color(t.text_dim)
            .small(),
    );
    if render.is_none() {
        let sel = selection_span(&app.session).filter(|_| !app.session.sel.is_collapsed());
        ui.horizontal(|ui| {
            ui.label("Placeholder");
            ui.add(egui::TextEdit::singleline(&mut st.label).hint_text("client.name").desired_width(140.0));
        });
        if ui.add_enabled(sel.is_some(), egui::Button::new("Mark the selection as client data")).clicked()
            && let Some(sp) = sel
        {
            let mut params = span_params(&app.session, &sp);
            if !st.label.trim().is_empty() {
                params["label"] = json!(st.label.trim());
            }
            acts.push(Act::Run("agent.private", params));
            st.label.clear();
        }
    }
    ui.add_space(6.0);
    let paras = anchor::paragraphs(&app.session.doc);
    for pr in &l.private {
        let sp = span_of(&app.session, &paras, l, &pr.anchor);
        card(ui, t, p.private, false, |ui| {
            ui.label(RichText::new(mask::placeholder(&pr.label)).color(p.private).font(theme::semibold(12.0)));
            let fallback = l.anchor(&pr.anchor).map(|a| a.quote.clone()).unwrap_or_default();
            quote_link(app, ui, t, sp.clone(), &fallback, &mut acts);
            if render.is_none()
                && let Some(sp) = sp
                && ui.small_button("Unmark").clicked()
            {
                let mut params = span_params(&app.session, &sp);
                params["on"] = json!(false);
                acts.push(Act::Run("agent.private", params));
            }
        });
    }
    st.acts.extend(acts);
}

fn history_tab(ui: &mut Ui, l: &Layer, t: &Tokens, p: &Palette) {
    if l.log.is_empty() {
        ui.label(RichText::new("Who changed what, and why: your edits and your agent's, with the reasons and sources it gave.").color(t.text_dim));
    }
    for e in l.log.iter().rev().take(300) {
        let (who, color) = match e.actor {
            Actor::Agent => ("Agent".to_string(), p.agent),
            Actor::Human => (format!("● {}", if e.who.is_empty() { "Person" } else { e.who.as_str() }), p.person),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(who).color(color).font(theme::semibold(11.5)));
            ui.label(RichText::new(e.kind.as_str()).color(t.text_disabled).small());
            ui.label(RichText::new(ago(e.at)).color(t.text_disabled).small());
        });
        ui.label(&e.text);
        if !e.source.is_empty() {
            ui.label(RichText::new(format!("based on: {}", e.source)).color(t.text_dim).small());
        }
        ui.add_space(4.0);
        ui.separator();
    }
}

fn connect_tab(ui: &mut Ui, t: &Tokens) {
    let url = crate::server_url().unwrap_or("http://127.0.0.1:7982/mcp");
    let base = url.trim_end_matches("/mcp");
    ui.label(RichText::new("Connect your agent").font(theme::semibold(13.0)));
    ui.label(
        RichText::new(
            "WordCraft runs an MCP server for your own agent (Claude Code, Claude Desktop or any MCP client). It only listens on this computer.",
        )
        .color(t.text_dim)
        .small(),
    );
    ui.add_space(6.0);
    let mono = |ui: &mut Ui, s: &str| {
        egui::Frame::NONE.fill(t.input).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(s).monospace().size(11.5)).wrap());
        });
    };
    ui.label("Claude Code:");
    mono(ui, &format!("claude mcp add --transport http wordcraft {url}"));
    ui.label("Watch for questions in the background:");
    mono(ui, &format!("curl -sN {base}/events"));
    ui.label("Check that it's running:");
    mono(ui, &format!("curl -s {base}/health"));
    ui.add_space(6.0);
    ui.label(RichText::new("Tools: get_context, read_document, list_questions, wait_for_questions, claim_question, answer_question, add_note, log_decision, run_command.").color(t.text_dim).small());
}
