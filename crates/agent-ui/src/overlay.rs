//! What the agent layer paints on the pages (`~/aitocraft/TOPICS.md` §1): a tiny numbered circle in
//! the margin beside each place ("3", or "3.2" for a topic's second place), coloured by the topic's
//! status, blue and pulsing while the agent works on one of its questions, with a dot when an
//! answer hasn't been read. Nothing else on the page, except client data (shaded, with its
//! placeholder on hover) and the places of the topic open in the pane (a light outline).

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use wordcraft_agent::layer::{QuestionStatus, StatusKind};
use wordcraft_agent::{mask, topics};
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::ext::PageGeometry;
use wordcraft_ui_egui::theme::Tokens;

use crate::{Act, AgentUi, Tab, palette, span_of};

/// The colour of a status.
pub(crate) fn status_color(k: StatusKind, p: &crate::Palette, t: &Tokens) -> Color32 {
    match k {
        StatusKind::Open => p.pending,
        StatusKind::Revise => Color32::from_rgb(0xD1, 0x34, 0x38),
        StatusKind::Agreed => p.answered,
        StatusKind::Info => t.text_disabled,
    }
}

/// A topic's marker state: its colour, whether the agent works on it, whether an answer is unread.
pub(crate) fn marker_state(app: &WordApp, topic: &str, t: &Tokens) -> (Color32, bool, bool) {
    let p = palette(t);
    let l = wordcraft_agent::layer(&app.session);
    let kind = l.topic(topic).map(|x| x.status.kind).unwrap_or_default();
    let qs = l.questions.iter().filter(|q| q.topic == topic && !q.hidden);
    let (mut working, mut unread) = (false, false);
    for q in qs {
        working |= matches!(q.status, QuestionStatus::Working | QuestionStatus::Transcribing);
        unread |= q.status == QuestionStatus::Answered && !q.seen;
    }
    (status_color(kind, &p, t), working, unread)
}

/// Draws one marker: a filled circle with the label.
pub(crate) fn marker(painter: &Painter, c: Pos2, label: &str, color: Color32, working: bool, unread: bool, time: f64, t: &Tokens) -> Rect {
    let font = FontId::proportional(if label.len() > 2 { 8.5 } else { 9.5 });
    let w = painter.layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE).size().x;
    let r = (w / 2.0 + 3.5).max(7.5);
    let p = palette(t);
    let fill = if working { p.working } else { color };
    if working {
        // Pulsing ring while the agent works.
        let phase = (time * 2.5).sin() as f32 * 0.5 + 0.5;
        painter.circle_stroke(
            c,
            r + 1.5 + phase * 2.5,
            Stroke::new(1.5, Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), (160.0 * (1.0 - phase)) as u8)),
        );
    }
    let rect = Rect::from_center_size(c, vec2(r * 2.0, r * 2.0));
    painter.rect_filled(rect, CornerRadius::same(r as u8), fill);
    painter.text(c, Align2::CENTER_CENTER, label, font, Color32::WHITE);
    if unread {
        painter.circle_filled(pos2(rect.right() - 1.5, rect.top() + 1.5), 3.0, p.answered);
        painter.circle_stroke(pos2(rect.right() - 1.5, rect.top() + 1.5), 3.0, Stroke::new(1.0, Color32::WHITE));
    }
    rect
}

pub(crate) fn paint(st: &mut AgentUi, app: &WordApp, ui: &Ui, painter: &Painter, g: &PageGeometry) {
    let s = &app.session;
    let l = wordcraft_agent::layer(s);
    let render = wordcraft_agent::render_state(s);
    if l.is_empty() && render.is_none() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let p = palette(&t);
    let paras = wordcraft_agent::anchor::paragraphs(&s.doc);
    let visible = |r: &Rect| r.intersects(g.clip);

    // Client data: shaded, with its placeholder on hover. In a template preview, the values filled in.
    let mut private: Vec<(Vec<(usize, Rect)>, String)> = Vec::new();
    for f in render.map(|r| r.filled.as_slice()).unwrap_or_default() {
        let a = wordcraft_doc::Pos::new(wordcraft_doc::StoryRef::Body, f.path.clone(), f.start);
        let b = wordcraft_doc::Pos::new(wordcraft_doc::StoryRef::Body, f.path.clone(), f.end);
        private.push((g.range_rects(app, &a, &b), mask::placeholder(&f.label)));
    }
    for pr in &l.private {
        if let Some(sp) = span_of(s, &paras, &l, &pr.anchor) {
            private.push((g.range_rects(app, &sp.start_pos(), &sp.end_pos()), mask::placeholder(&pr.label)));
        }
    }
    let fill = Color32::from_rgba_unmultiplied(p.private.r(), p.private.g(), p.private.b(), 30);
    for (rs, label) in &private {
        for (_, r) in rs.iter().filter(|(_, r)| visible(r)) {
            painter.rect_filled(*r, CornerRadius::same(2), fill);
        }
        if let Some((_, first)) = rs.first()
            && g.pointer.is_some_and(|pt| rs.iter().any(|(_, r)| r.contains(pt)))
        {
            let font = FontId::proportional(9.5);
            let galley = painter.layout_no_wrap(format!("client data · agent sees {label}"), font, Color32::WHITE);
            let at = pos2(first.left(), first.top() - galley.size().y - 4.0);
            painter.rect_filled(Rect::from_min_size(at, galley.size()).expand2(vec2(4.0, 1.5)), CornerRadius::same(3), p.private);
            painter.galley(at, galley, Color32::WHITE);
        }
    }

    // A client's copy: the paragraphs the person edited there, marked in the left margin.
    for (block, id, conflict) in wordcraft_agent::copyedit::edited_blocks(s) {
        let paths: Vec<_> = paras.iter().filter(|p| p.0.first() == Some(&(block as u32))).cloned().collect();
        let (Some(first), Some(last)) = (paths.first(), paths.last()) else { continue };
        let a = wordcraft_doc::Pos::new(wordcraft_doc::StoryRef::Body, first.clone(), 0);
        let b = wordcraft_doc::Pos::new(
            wordcraft_doc::StoryRef::Body,
            last.clone(),
            wordcraft_agent::anchor::para_text(&s.doc, last).map_or(0, str::len),
        );
        let rects = g.range_rects(app, &a, &b);
        let Some((page, top)) = rects.first().copied() else { continue };
        let bottom = rects.iter().filter(|(pg, _)| *pg == page).map(|(_, r)| r.bottom()).fold(top.bottom(), f32::max);
        let Some(pr) = g.pages().get(page).copied() else { continue };
        let color = if conflict { Color32::from_rgb(0xD1, 0x34, 0x38) } else { p.agent };
        let bar = Rect::from_min_max(pos2(pr.left() + 30.0, top.top()), pos2(pr.left() + 33.0, bottom));
        if !visible(&bar) {
            continue;
        }
        painter.rect_filled(bar, CornerRadius::same(1), color);
        let resp = ui.interact(bar.expand2(vec2(5.0, 0.0)), ui.id().with(("agent_copy_edit", id.as_str())), Sense::click());
        let tip = if conflict {
            "Your edit in this copy. The template's paragraph changed since: your text is kept. Click to move it to the template or discard it."
        } else {
            "Your edit in this copy only (other clients and the template don't have it). Click to move it to the template or discard it."
        };
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
        egui::Popup::menu(&resp).show(|ui| {
            if ui.button("Move to template").on_hover_text("Every client's copy gets this text").clicked() {
                st.acts.push(Act::Run("agent.moveToTemplate", serde_json::json!({"id": id})));
                ui.close();
            }
            if ui.button("Discard this edit").on_hover_text("The copy shows the template's text again").clicked() {
                st.acts.push(Act::Run("agent.discardCopyEdit", serde_json::json!({"id": id})));
                ui.close();
            }
        });
    }

    // Places: the open topic's outlined; every place's marker in the margin.
    let all = topics::resolve(s);
    let time = ui.input(|i| i.time);
    let mut slots: Vec<(usize, f32, f32)> = Vec::new();
    let mut any_working = false;
    for topic in all.iter() {
        let (color, working, unread) = marker_state(app, &topic.id, &t);
        any_working |= working;
        let open = st.open_topic.as_deref() == Some(topic.id.as_str());
        for pl in &topic.places {
            // Its repetitions too, in a client's copy (a place inside a template loop).
            for (k, (a, b)) in pl.ranges().enumerate() {
                let rects = if pl.removed.is_some() || a >= b { g.caret_rect(app, a).into_iter().collect() } else { g.range_rects(app, a, b) };
                if open {
                    for (_, r) in rects.iter().filter(|(_, r)| visible(r)) {
                        painter.rect_stroke(r.expand(1.0), CornerRadius::same(2), Stroke::new(1.0, color), egui::StrokeKind::Outside);
                    }
                }
                let Some((page, first)) = rects.first().copied() else { continue };
                let Some(pr) = g.pages().get(page).copied() else { continue };
                // In the right margin, stacked when several share a line.
                let y = first.center().y.max(first.top() + 7.0);
                let mut x = pr.right() - 22.0;
                while slots.iter().any(|(pg, sx, sy)| *pg == page && (sx - x).abs() < 16.0 && (sy - y).abs() < 15.0) {
                    x -= 18.0;
                }
                slots.push((page, x, y));
                if !g.clip.contains(pos2(x, y)) {
                    continue;
                }
                let rect = marker(painter, pos2(x, y), &pl.label, color, working, unread, time, &t);
                if pl.removed.is_some() {
                    painter.line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.2, Color32::WHITE));
                }
                let resp = ui.interact(rect.expand(2.0), ui.id().with(("agent_marker", pl.id.as_str(), k)), Sense::click());
                let tip = format!("Topic {}{}", topic.number, if pl.removed.is_some() { " · the text here was removed" } else { "" });
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
                if resp.clicked() {
                    st.acts.push(Act::GoToRange(a.clone(), b.clone()));
                    st.acts.push(Act::Open(Tab::Selection, Some(topic.id.clone())));
                }
            }
        }
    }
    if any_working {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
    }
}
