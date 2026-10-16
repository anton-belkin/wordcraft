//! The Agent pane (`~/aitocraft/TOPICS.md` §3): three tabs.
//!
//! - **Selection**: what is selected, then the topics touching it as cards (number and status with
//!   its text, places, notes, the discussion folded once a status is set), and the question box at
//!   the very bottom. Asking on a selection that has a topic adds to it; "New topic" starts another.
//! - **Overview**: every topic in document order, with the sign-off line and filters.
//! - **History**: who changed what, and why.

use egui::{Color32, RichText, Ui, vec2};
use serde_json::json;
use wordcraft_agent::layer::{Actor, Layer, Note, QuestionStatus, StatusKind, Topic};
use wordcraft_agent::links::{self, Seg};
use wordcraft_agent::topics::{self, RTopic};
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::theme::{self, Tokens};

use crate::overlay::{marker, marker_state, status_color};
use crate::{Act, AgentUi, Palette, Tab, ago, clip, palette, selection_span, span_params, span_text};

pub(crate) fn show(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let p = palette(&t);
    let max = (ui.available_width() / 2.0).max(360.0);
    let mut close = false;
    egui::Panel::right("agent_pane")
        .default_size(380.0)
        .min_size(320.0)
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
            let all = topics::resolve(&app.session);
            ui.horizontal(|ui| {
                for (tab, name) in [
                    (Tab::Selection, "Selection".to_string()),
                    (Tab::Overview, format!("Overview ({})", all.len())),
                    (Tab::History, "History".into()),
                ] {
                    if ui.selectable_label(st.tab == tab, name).clicked() {
                        st.tab = tab;
                    }
                }
            });
            ui.separator();
            let l = wordcraft_agent::layer(&app.session);
            match st.tab {
                Tab::Selection => {
                    // The question box at the very bottom; the cards above it scroll.
                    egui::Panel::bottom("agent_ask_box")
                        .frame(egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 0 }))
                        .show(ui, |ui| {
                            ask_box(st, app, ui, &t, &p);
                        });
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| selection_tab(st, app, ui, &l, &all, &t, &p));
                }
                Tab::Overview => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| overview_tab(st, app, ui, &l, &all, &t, &p));
                }
                Tab::History => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| history_tab(ui, &l, &t, &p));
                }
            }
        });
    if close {
        st.open = false;
    }
}

/// A question's text; a marker `⟦n⟧` (a passage pointed at while speaking) is a numbered bubble
/// that selects that passage.
fn spoken_text(app: &WordApp, ui: &mut Ui, l: &Layer, q: &wordcraft_agent::layer::Question, acts: &mut Vec<Act>) {
    if q.refs.is_empty() {
        if !q.text.is_empty() {
            ui.label(&q.text);
        }
        return;
    }
    let p = palette(&Tokens::get(ui.ctx()));
    let all = topics::resolve(&app.session);
    let place = |id: &str| all.iter().flat_map(|t| t.places.iter()).find(|x| x.id == id).cloned();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let mut rest = q.text.as_str();
        loop {
            let Some(i) = rest.find('⟦') else {
                if !rest.trim().is_empty() {
                    ui.label(rest.trim());
                }
                break;
            };
            let (before, after) = rest.split_at(i);
            if !before.trim().is_empty() {
                ui.label(before.trim());
            }
            let after = after.get('⟦'.len_utf8()..).unwrap_or("");
            let Some(j) = after.find('⟧') else { break };
            let n: u32 = after.get(..j).unwrap_or("").trim().parse().unwrap_or(0);
            rest = after.get(j + '⟧'.len_utf8()..).unwrap_or("");
            let pl = q.refs.iter().find(|r| r.n == n).and_then(|r| place(&r.place));
            let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 14.0), egui::Sense::click());
            crate::voice_bubble(ui.painter(), rect.min, n, p.person);
            let tip = pl.as_ref().map_or_else(
                || "The text was deleted".to_string(),
                |x| format!("Place {}: “{}”", x.label, clip(&topics::place_text(&app.session, x), 60)),
            );
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
            if resp.clicked()
                && let Some(x) = pl
                && let (Some(a), Some(b)) = (x.from, x.to)
            {
                acts.push(Act::GoToRange(a, b));
            }
        }
    });
    let _ = l;
}

/// A text with its links (current numbers, clickable).
fn rich(ui: &mut Ui, app: &WordApp, text: &str, t: &Tokens, color: Color32, acts: &mut Vec<Act>) {
    let segs = links::render(&app.session, text);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        for seg in segs {
            match seg {
                Seg::Text(s) => {
                    ui.label(RichText::new(s).color(color));
                }
                Seg::Link { label, target } => {
                    let tip = match &target {
                        links::Target::Url(u) => u.clone(),
                        links::Target::Gone => "no longer in the document".into(),
                        _ => "Show it".into(),
                    };
                    if matches!(target, links::Target::Gone) {
                        ui.label(RichText::new(label).color(t.text_disabled).strikethrough());
                    } else if ui.link(RichText::new(label).color(t.accent_text)).on_hover_text(tip).clicked() {
                        acts.push(Act::Link(target));
                    }
                }
            }
        }
    });
}

fn chip(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 34))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).font(theme::semibold(11.0)));
        });
}

/// The agent's proposed changes: the options, Apply option X, Reject (with a comment).
fn edits(st: &mut AgentUi, app: &WordApp, ui: &mut Ui, topic: &Topic, t: &Tokens, p: &Palette, acts: &mut Vec<Act>) {
    use wordcraft_agent::layer::ProposalState;
    let Some(e) = &topic.edits else { return };
    let tint = |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 22);
    match e.state {
        ProposalState::Applied => {
            let title = e.options.iter().find(|o| o.option == e.chosen).map(|o| o.title.clone()).unwrap_or_default();
            ui.label(
                RichText::new(format!("Applied option {}{}", e.chosen, if title.is_empty() { String::new() } else { format!(": {title}") }))
                    .color(p.answered)
                    .small(),
            );
            return;
        }
        ProposalState::Rejected => {
            let c = if e.comment.is_empty() { String::new() } else { format!(": “{}”", clip(&e.comment, 80)) };
            ui.label(RichText::new(format!("You rejected the proposal{c}")).color(t.text_dim).small());
            return;
        }
        ProposalState::Outdated => {
            egui::Frame::NONE.fill(tint(p.failed)).corner_radius(6.0).inner_margin(6).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("The agent's proposal is out of date, ask the agent to redo").color(p.failed).font(theme::semibold(11.5)));
            });
            return;
        }
        ProposalState::Pending => {}
    }
    let chosen = st
        .option_of
        .get(&topic.id)
        .filter(|o| e.options.iter().any(|x| &x.option == *o))
        .cloned()
        .or_else(|| e.options.first().map(|o| o.option.clone()))
        .unwrap_or_default();
    let Some(opt) = e.options.iter().find(|o| o.option == chosen) else { return };
    egui::Frame::NONE.fill(tint(p.agent)).corner_radius(6.0).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            crate::sparkle(ui, p.agent, 12.0);
            let n = opt.steps.len();
            ui.label(
                RichText::new(format!("The agent proposes {n} change{}", if n == 1 { "" } else { "s" })).color(p.agent).font(theme::semibold(12.0)),
            );
        });
        // The options, side by side: switching changes the preview in the document.
        if e.options.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for o in &e.options {
                    let label = if o.title.is_empty() { format!("Option {}", o.option) } else { format!("{} · {}", o.option, clip(&o.title, 24)) };
                    if ui.selectable_label(o.option == chosen, RichText::new(label).size(11.5)).clicked() {
                        st.option_of.insert(topic.id.clone(), o.option.clone());
                    }
                }
            });
        }
        if !opt.title.is_empty() {
            ui.label(RichText::new(&opt.title).font(theme::semibold(12.0)));
        }
        if !opt.why.is_empty() {
            rich(ui, app, &opt.why, t, t.text_dim, acts);
        }
        for step in &opt.steps {
            ui.label(RichText::new(format!("• {}", clip(&step.summary, 140))).size(11.5).color(t.text));
        }
        let previewing =
            wordcraft_agent::render_state(&app.session).and_then(|r| r.proposal.clone()).is_some_and(|(tp, o)| tp == topic.id && o == chosen);
        let mut show = !st.no_preview;
        if ui
            .checkbox(&mut show, RichText::new(if previewing { "Shown in the document (read only)" } else { "Show in the document" }).size(11.0))
            .changed()
        {
            st.no_preview = !show;
            if show {
                st.open_topic = Some(topic.id.clone());
            }
        }
        ui.horizontal(|ui| {
            let apply = egui::Button::new(RichText::new(format!("Apply option {chosen}")).strong().color(Color32::WHITE)).fill(p.agent);
            if ui.add(apply).on_hover_text("Make these changes in the document: one step you can undo").clicked() {
                acts.push(Act::Run("agent.applyProposal", json!({"topic": topic.id, "option": chosen})));
            }
            if ui.button("Reject").clicked() {
                st.rejecting = if st.rejecting.as_deref() == Some(topic.id.as_str()) { None } else { Some(topic.id.clone()) };
            }
        });
        if st.rejecting.as_deref() == Some(topic.id.as_str()) {
            ui.add(
                egui::TextEdit::multiline(&mut st.reject_text)
                    .hint_text("Why (optional): the agent gets it as a follow-up")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.button("Reject the proposal").clicked() {
                    acts.push(Act::Run("agent.rejectProposal", json!({"topic": topic.id, "comment": st.reject_text.trim()})));
                    st.reject_text.clear();
                    st.rejecting = None;
                }
                if ui.small_button("Cancel").clicked() {
                    st.rejecting = None;
                }
            });
        }
    });
}

/// The topic's number as a marker, the same as on the page.
fn number(ui: &mut Ui, app: &WordApp, topic: &str, n: usize, t: &Tokens) {
    let (color, working, unread) = marker_state(app, topic, t);
    let (r, _) = ui.allocate_exact_size(vec2(22.0, 18.0), egui::Sense::hover());
    marker(ui.painter(), r.center(), &n.to_string(), color, working, unread, ui.input(|i| i.time), t);
}

fn selection_tab(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui, l: &Layer, all: &[RTopic], t: &Tokens, p: &Palette) {
    let mut acts = vec![];
    let (a, b) = app.session.sel.ordered();
    // What is selected.
    let sp = selection_span(&app.session);
    ui.label(RichText::new("Selected").color(t.text_dim).small());
    match &sp {
        Some(sp) if !app.session.sel.is_collapsed() => {
            ui.label(RichText::new(format!("“{}”", clip(span_text(&app.session, sp).trim(), 220))).italics());
        }
        Some(sp) => {
            ui.label(RichText::new(format!("Paragraph {} (the caret is here)", sp.para)).color(t.text_dim));
        }
        None => {
            ui.label(RichText::new("Select a passage in the document").color(t.text_disabled));
        }
    }
    ui.add_space(6.0);
    // The topics touching it, and the one opened from a marker or a link.
    let mut shown: Vec<RTopic> = topics::touching(&app.session, &a, &b);
    if let Some(open) = &st.open_topic
        && !shown.iter().any(|x| &x.id == open)
        && let Some(x) = all.iter().find(|x| &x.id == open)
    {
        shown.insert(0, x.clone());
    }
    if shown.is_empty() {
        ui.label(RichText::new("Nothing about this passage yet. Ask below: the question becomes a topic, numbered in the margin.").color(t.text_dim));
    }
    for rt in &shown {
        if let Some(topic) = l.topic(&rt.id) {
            topic_card(st, app, ui, l, rt, topic, t, p, &mut acts);
        }
    }
    st.acts.extend(acts);
}

#[allow(clippy::too_many_arguments)]
fn topic_card(st: &mut AgentUi, app: &WordApp, ui: &mut Ui, l: &Layer, rt: &RTopic, topic: &Topic, t: &Tokens, p: &Palette, acts: &mut Vec<Act>) {
    let color = status_color(topic.status.kind, p, t);
    let open = st.open_topic.as_deref() == Some(topic.id.as_str());
    let stroke = if open { egui::Stroke::new(1.5, color) } else { egui::Stroke::new(1.0, t.border) };
    egui::Frame::NONE.fill(t.input).corner_radius(6.0).inner_margin(10).stroke(stroke).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Number and status.
        ui.horizontal(|ui| {
            number(ui, app, &topic.id, rt.number, t);
            ui.label(RichText::new(format!("Topic {}", rt.number)).font(theme::semibold(13.0)));
            chip(ui, topic.status.kind.label(), color);
            if topic.status.set {
                let who = if topic.status.by == Actor::Agent { "agent".to_string() } else { topic.status.who.clone() };
                ui.label(RichText::new(format!("{who} · {}", ago(topic.status.at))).color(t.text_disabled).small());
            }
        });
        if !topic.status.text.is_empty() {
            rich(ui, app, &topic.status.text, t, t.text, acts);
        }
        // The person sets the status (Agreed only by a person).
        ui.horizontal_wrapped(|ui| {
            for k in StatusKind::ALL {
                let c = status_color(k, p, t);
                let on = topic.status.kind == k && topic.status.set;
                let b = egui::Button::new(RichText::new(k.label()).color(if on { Color32::WHITE } else { c }).size(11.0))
                    .fill(if on { c } else { Color32::TRANSPARENT })
                    .stroke(egui::Stroke::new(1.0, c))
                    .corner_radius(8.0);
                if ui.add(b).clicked() {
                    let draft = st.status_draft.remove(&topic.id).unwrap_or_default();
                    let text = if draft.trim().is_empty() { links::for_agent(&app.session, &topic.status.text) } else { draft };
                    acts.push(Act::Run("agent.setStatus", json!({"topic": topic.id, "status": k.id(), "text": text})));
                }
            }
        });
        let draft = st.status_draft.entry(topic.id.clone()).or_default();
        ui.add(
            egui::TextEdit::singleline(draft)
                .hint_text("status in a sentence, then pick the status")
                .desired_width(f32::INFINITY)
                .font(theme::regular(11.5)),
        );
        // The agent's proposal.
        if let Some(pr) = &topic.proposal {
            egui::Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(p.answered.r(), p.answered.g(), p.answered.b(), 26))
                .corner_radius(6.0)
                .inner_margin(6)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Agent suggests: Agreed").color(p.answered).font(theme::semibold(11.5)));
                    rich(ui, app, &pr.reasons, t, t.text, acts);
                    ui.horizontal(|ui| {
                        if ui.button("Confirm Agreed").clicked() {
                            acts.push(Act::Run("agent.confirmAgreed", json!({"topic": topic.id})));
                        }
                        if ui.small_button("Dismiss").clicked() {
                            acts.push(Act::Run("agent.dismissProposal", json!({"topic": topic.id})));
                        }
                    });
                });
        }
        edits(st, app, ui, topic, t, p, acts);
        ui.add_space(4.0);
        // Places.
        ui.label(RichText::new("Places").color(t.text_dim).small());
        for pl in &rt.places {
            ui.horizontal_wrapped(|ui| {
                let label = RichText::new(&pl.label).color(t.accent_text).font(theme::semibold(11.5));
                if ui.link(label).on_hover_text("Show it").clicked()
                    && let (Some(a), Some(b)) = (&pl.from, &pl.to)
                {
                    acts.push(Act::GoToRange(a.clone(), b.clone()));
                }
                let text = topics::place_text(&app.session, pl);
                if pl.removed.is_some() {
                    ui.label(RichText::new(format!("removed here: “{}”", clip(text.trim(), 80))).strikethrough().color(t.text_dim));
                } else {
                    ui.label(RichText::new(format!("“{}”", clip(text.trim(), 80))).italics());
                }
                if pl.added_by == Actor::Agent && ui.small_button("Reject").on_hover_text("Take this place off the topic").clicked() {
                    acts.push(Act::Run("agent.rejectPlace", json!({"place": pl.id})));
                }
            });
            if !pl.reason.is_empty() {
                ui.label(RichText::new(format!("    {}", pl.reason)).color(t.text_dim).small());
            }
        }
        // (Places are marked on the template, not on a client's copy.)
        let sel_in = !app.session.sel.is_collapsed() && wordcraft_agent::render_state(&app.session).is_none() && {
            let (a, b) = app.session.sel.ordered();
            !rt.places.iter().any(|pl| pl.ranges().any(|(f, x)| *f <= b && a <= *x))
        };
        if sel_in && ui.small_button("Add the selection as a place").clicked() {
            acts.push(Act::Run("agent.addPlace", json!({"topic": topic.id})));
        }
        // Notes.
        let mut notes: Vec<&Note> = l.notes.iter().filter(|n| n.topic == topic.id).collect();
        notes.sort_by_key(|n| (!n.pinned, n.at));
        if !notes.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Notes").color(t.text_dim).small());
        }
        for n in notes {
            let (icon, kind) = crate::kind_label(&n.kind);
            let title = if n.title.is_empty() { kind.to_string() } else { n.title.clone() };
            let pin = if n.pinned { "● " } else { "" };
            let head = RichText::new(format!("{pin}{icon}  {title}")).color(p.note).font(theme::semibold(11.5));
            let want_open = st.open_note.as_deref() == Some(n.id.as_str());
            egui::CollapsingHeader::new(head)
                .id_salt(("agent_note", n.id.as_str()))
                .default_open(n.pinned || want_open)
                .open(want_open.then_some(true))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("{kind} · {}", if n.author == Actor::Agent { "by your agent" } else { "by a person" }))
                            .color(t.text_disabled)
                            .small(),
                    );
                    rich(ui, app, &n.text, t, t.text, acts);
                    for k in &n.links {
                        if ui.link(RichText::new(format!("↗ {}", k.label)).color(t.accent_text)).on_hover_text(&k.href).clicked() {
                            acts.push(Act::Link(links::Target::Url(k.href.clone())));
                        }
                        if !k.summary.is_empty() {
                            ui.label(RichText::new(&k.summary).color(t.text_dim).small());
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button(if n.pinned { "Unpin" } else { "Pin" }).clicked() {
                            acts.push(Act::Run("agent.pinNote", json!({"id": n.id})));
                        }
                        if ui.small_button("Remove").clicked() {
                            acts.push(Act::Run("agent.removeNote", json!({"id": n.id})));
                        }
                    });
                });
        }
        if st.open_note.is_some() {
            st.open_note = None;
        }
        // The discussion: folded once a status is set.
        let mut qs: Vec<_> = l.questions.iter().filter(|q| q.topic == topic.id && !q.hidden).collect();
        qs.sort_by_key(|q| q.asked_at);
        if !qs.is_empty() {
            ui.add_space(4.0);
            let unread = qs.iter().filter(|q| q.status == QuestionStatus::Answered && !q.seen).count();
            let head = if unread > 0 { format!("Discussion ({}) · {unread} new", qs.len()) } else { format!("Discussion ({})", qs.len()) };
            let r = egui::CollapsingHeader::new(RichText::new(head).color(t.text_dim).font(theme::semibold(11.5)))
                .id_salt(("agent_discussion", topic.id.as_str()))
                .default_open(!topic.status.set || unread > 0)
                .show(ui, |ui| {
                    for q in &qs {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(if q.asked_by.is_empty() { "You".to_string() } else { q.asked_by.clone() })
                                    .color(p.person)
                                    .font(theme::semibold(11.0)),
                            );
                            ui.label(RichText::new(ago(q.asked_at)).color(t.text_disabled).small());
                            match q.status {
                                QuestionStatus::Pending => chip(ui, "waiting for the agent", p.pending),
                                QuestionStatus::Working => chip(ui, "the agent is on it", p.working),
                                QuestionStatus::Transcribing => chip(ui, "transcribing what you said…", p.transcribing),
                                QuestionStatus::Failed => chip(ui, "not transcribed", p.failed),
                                QuestionStatus::Answered => {}
                            }
                            if q.spoken {
                                ui.label(RichText::new("· spoken").small().color(t.text_dim));
                            }
                        });
                        if q.status == QuestionStatus::Failed {
                            ui.label(RichText::new(&q.error).small().color(p.failed));
                            ui.horizontal(|ui| {
                                if ui.small_button("Retry").on_hover_text("Transcribe the recording again").clicked() {
                                    acts.push(Act::Voice(crate::VoiceAct::Retry(q.id.clone())));
                                }
                                if ui.small_button("Dismiss").on_hover_text("Drop this question and its recording").clicked() {
                                    acts.push(Act::Voice(crate::VoiceAct::Dismiss(q.id.clone())));
                                }
                            });
                        }
                        spoken_text(app, ui, l, q, acts);
                        if let Some(a) = &q.answer {
                            ui.horizontal(|ui| {
                                crate::sparkle(ui, p.agent, 11.0);
                                ui.label(RichText::new("Your agent").color(p.agent).font(theme::semibold(11.0)));
                            });
                            rich(ui, app, &a.text, t, t.text, acts);
                            for k in &a.links {
                                if ui.link(RichText::new(format!("↗ {}", k.label)).color(t.accent_text)).clicked() {
                                    acts.push(Act::Link(links::Target::Url(k.href.clone())));
                                }
                            }
                        }
                        ui.add_space(4.0);
                    }
                });
            // Read once shown.
            if r.body_returned.is_some() {
                for q in qs.iter().filter(|q| q.status == QuestionStatus::Answered && !q.seen) {
                    acts.push(Act::Run("agent.seen", json!({"id": q.id})));
                }
            }
        }
    });
    ui.add_space(8.0);
}

fn ask_box(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui, t: &Tokens, p: &Palette) {
    let mut acts = vec![];
    let preview = wordcraft_agent::render_state(&app.session).is_some();
    let sp = st
        .ask_span
        .clone()
        .filter(|sp| wordcraft_agent::anchor::para_text(&app.session.doc, &sp.path).is_some_and(|x| x.len() >= sp.end))
        .or_else(|| selection_span(&app.session));
    let (a, b) = app.session.sel.ordered();
    let topic_here = if st.ask_span.is_none() { topics::touching(&app.session, &a, &b).first().map(|x| (x.id.clone(), x.number)) } else { None };
    ui.separator();
    let about = match (&topic_here, &sp) {
        (Some((_, n)), _) if !st.new_topic => format!("Ask in topic {n}"),
        (_, Some(sp)) if !app.session.sel.is_collapsed() || st.ask_span.is_some() => {
            format!("Ask about “{}”", clip(span_text(&app.session, sp).trim(), 50))
        }
        (_, Some(sp)) => format!("Ask about paragraph {}", sp.para),
        _ => "Select a passage to ask about it".into(),
    };
    ui.label(RichText::new(about).color(t.text_dim).small());
    let edit = egui::TextEdit::multiline(&mut st.draft)
        .hint_text("Your question. The agent answers in the background; keep reading.")
        .desired_rows(2)
        .desired_width(f32::INFINITY);
    let r = ui.add(edit);
    if std::mem::take(&mut st.focus_draft) {
        r.request_focus();
    }
    let send_key = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.horizontal(|ui| {
        let can = sp.is_some() && !st.draft.trim().is_empty();
        let b = egui::Button::new(RichText::new("Ask").strong().color(Color32::WHITE)).fill(p.agent);
        if (ui.add_enabled(can, b).clicked() || (send_key && can))
            && let Some(sp) = &sp
        {
            let mut params = if st.ask_span.is_some() { span_params(&app.session, sp) } else { json!({}) };
            params["text"] = json!(st.draft.trim());
            if st.new_topic {
                params["newTopic"] = json!(true);
            }
            acts.push(Act::Run("agent.ask", params));
            st.draft.clear();
            st.ask_span = None;
            st.new_topic = false;
        }
        if topic_here.is_some() {
            ui.checkbox(&mut st.new_topic, RichText::new("New topic").size(11.5)).on_hover_text("Start a separate topic on this selection");
        }
        ui.label(RichText::new(if cfg!(target_os = "macos") { "Cmd+Enter" } else { "Ctrl+Enter" }).color(t.text_disabled).small());
    });
    let proposal_shown = wordcraft_agent::render_state(&app.session).is_some_and(|r| r.proposal.is_some());
    if let Some(w) = &st.asked_where {
        ui.label(RichText::new(w).color(t.text_dim).small());
    } else if proposal_shown {
        ui.label(
            RichText::new("You are looking at a proposal (read only): apply or reject it, or untick Show in the document to ask.")
                .color(t.text_dim)
                .small(),
        );
    } else if preview {
        ui.label(RichText::new("You are reading a client's copy: questions go on the template it was rendered from.").color(t.text_dim).small());
    }
    if !preview || !st.draft.is_empty() {
        st.asked_where = None;
    }
    st.acts.extend(acts);
}

fn overview_tab(st: &mut AgentUi, app: &mut WordApp, ui: &mut Ui, l: &Layer, all: &[RTopic], t: &Tokens, p: &Palette) {
    let so = wordcraft_agent::topic_cmds::sign_off(&app.session);
    ui.label(RichText::new(so["line"].as_str().unwrap_or("")).font(theme::semibold(14.0)));
    ui.label(RichText::new("Readiness for sign-off").color(t.text_dim).small());
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (key, label) in [
            (None, "All"),
            (Some("open"), "Open"),
            (Some("revise"), "Revise"),
            (Some("agreed"), "Agreed"),
            (Some("info"), "For information"),
            (Some("unread"), "Unread"),
            (Some("agent"), "With the agent"),
        ] {
            let on = st.filter.as_deref() == key;
            if ui.selectable_label(on, label).clicked() {
                st.filter = key.map(str::to_string);
            }
        }
    });
    ui.separator();
    let mut acts = vec![];
    for rt in all {
        let Some(topic) = l.topic(&rt.id) else { continue };
        let qs: Vec<_> = l.questions.iter().filter(|q| q.topic == rt.id && !q.hidden).collect();
        let unread = qs.iter().filter(|q| q.status == QuestionStatus::Answered && !q.seen).count();
        let with_agent = qs.iter().any(|q| q.status.is_open());
        let pass = match st.filter.as_deref() {
            None => true,
            Some("unread") => unread > 0,
            Some("agent") => with_agent,
            Some(k) => StatusKind::parse(k) == Some(topic.status.kind),
        };
        if !pass {
            continue;
        }
        let notes = l.notes.iter().filter(|n| n.topic == rt.id).count();
        let color = status_color(topic.status.kind, p, t);
        let resp = egui::Frame::NONE.inner_margin(4).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                number(ui, app, &rt.id, rt.number, t);
                chip(ui, topic.status.kind.label(), color);
                let preview = links::plain(&app.session, &topic.status.text);
                let first = rt.places.first().map(|pl| topics::place_text(&app.session, pl)).unwrap_or_default();
                let line = if preview.is_empty() { format!("“{}”", clip(first.trim(), 50)) } else { clip(&preview, 60) };
                ui.label(RichText::new(line).color(t.text));
            });
            let mut counts = vec![format!("{} place{}", rt.places.len(), if rt.places.len() == 1 { "" } else { "s" })];
            if notes > 0 {
                counts.push(format!("{notes} note{}", if notes == 1 { "" } else { "s" }));
            }
            if unread > 0 {
                counts.push(format!("{unread} unread"));
            }
            if with_agent {
                counts.push("with the agent".into());
            }
            ui.label(RichText::new(format!("      {}", counts.join(" · "))).color(t.text_dim).small());
        });
        let click = ui.interact(resp.response.rect, ui.id().with(("agent_overview", rt.id.as_str())), egui::Sense::click());
        if click.hovered() {
            ui.painter().rect_stroke(resp.response.rect, 4.0, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        }
        if click.clicked() {
            if let Some(pl) = rt.places.first()
                && let (Some(a), Some(b)) = (&pl.from, &pl.to)
            {
                acts.push(Act::GoToRange(a.clone(), b.clone()));
            }
            acts.push(Act::Open(Tab::Selection, Some(rt.id.clone())));
        }
    }
    if all.is_empty() {
        ui.label(RichText::new("No topics yet: select a passage and ask (Selection tab, or right-click ▸ Ask the Agent).").color(t.text_dim));
    }
    st.acts.extend(acts);
}

fn history_tab(ui: &mut Ui, l: &Layer, t: &Tokens, p: &Palette) {
    if l.log.is_empty() {
        ui.label(RichText::new("Who changed what, and why: your edits and your agent's, statuses and decisions.").color(t.text_dim));
    }
    for e in l.log.iter().rev().take(300) {
        let (who, color) = match e.actor {
            Actor::Agent => ("Agent".to_string(), p.agent),
            Actor::Human => (if e.who.is_empty() { "Person".to_string() } else { e.who.clone() }, p.person),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(who).color(color).font(theme::semibold(11.5)));
            ui.label(RichText::new(e.kind.as_str()).color(t.text_disabled).small());
            ui.label(RichText::new(ago(e.at)).color(t.text_disabled).small());
        });
        ui.label(&e.text);
        if !e.source.is_empty() && e.kind != "question" {
            ui.label(RichText::new(format!("based on: {}", e.source)).color(t.text_dim).small());
        }
        ui.add_space(4.0);
        ui.separator();
    }
}
