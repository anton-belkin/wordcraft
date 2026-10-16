//! What the agent layer paints on the pages: client data shaded with its placeholder, questions
//! underlined in their status colour with a badge in the margin, the agent's notes underlined with
//! a card in the margin (or a small tab when there is no room) and a hover card over the text.

use egui::{Color32, CornerRadius, FontId, Painter, Rect, Stroke, Ui, pos2, vec2};
use wordcraft_agent::anchor::{self, Span};
use wordcraft_agent::layer::QuestionStatus;
use wordcraft_agent::mask;
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::ext::PageGeometry;
use wordcraft_ui_egui::theme::{self, Tokens};

use crate::{Act, AgentUi, Tab, clip, kind_label, palette, span_of, status_color, status_text};

/// Narrowest room beside a page that takes margin cards.
const CARD_MIN: f32 = 150.0;
const CARD_MAX: f32 = 250.0;

/// A thing shown in the margin: its line on the page, what to draw, what a click does.
struct Margin {
    page: usize,
    y: f32,
    color: Color32,
    icon: String,
    title: String,
    body: String,
    act: Act,
    id: String,
}

fn underline(painter: &Painter, rects: &[(usize, Rect)], color: Color32, dotted: bool) {
    for (_, r) in rects {
        let y = r.bottom() - 0.5;
        if dotted {
            let mut x = r.left();
            while x < r.right() {
                painter.line_segment([pos2(x, y), pos2((x + 2.5).min(r.right()), y)], Stroke::new(1.5, color));
                x += 5.0;
            }
        } else {
            painter.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(2.0, color));
        }
    }
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
    let paras = anchor::paragraphs(&s.doc);
    let rects = |sp: &Span| g.range_rects(app, &sp.start_pos(), &sp.end_pos());
    let visible = |r: &Rect| r.intersects(g.clip);
    let tag_font = FontId::proportional(9.5);

    // Client data: shaded, with its placeholder on hover. In a template preview, the values filled in.
    let mut private: Vec<(Vec<(usize, Rect)>, String)> = Vec::new();
    for f in render.map(|r| r.filled.as_slice()).unwrap_or_default() {
        let sp = Span { para: 0, path: f.path.clone(), start: f.start, end: f.end };
        private.push((rects(&sp), mask::placeholder(&f.label)));
    }
    for pr in &l.private {
        if let Some(sp) = span_of(s, &paras, &l, &pr.anchor) {
            private.push((rects(&sp), mask::placeholder(&pr.label)));
        }
    }
    let fill = Color32::from_rgba_unmultiplied(p.private.r(), p.private.g(), p.private.b(), 34);
    for (rs, label) in &private {
        for (_, r) in rs.iter().filter(|(_, r)| visible(r)) {
            painter.rect_filled(*r, CornerRadius::same(2), fill);
        }
        underline(painter, rs, p.private, true);
        if let Some((_, first)) = rs.first()
            && g.pointer.is_some_and(|pt| rs.iter().any(|(_, r)| r.contains(pt)))
        {
            let galley = painter.layout_no_wrap(format!("client data · agent sees {label}"), tag_font.clone(), Color32::WHITE);
            let at = pos2(first.left(), first.top() - galley.size().y - 4.0);
            let bg = Rect::from_min_size(at, galley.size()).expand2(vec2(4.0, 1.5));
            painter.rect_filled(bg, CornerRadius::same(3), p.private);
            painter.galley(at, galley, Color32::WHITE);
        }
    }

    let mut margin: Vec<Margin> = Vec::new();
    // Questions.
    for q in &l.questions {
        let Some(sp) = span_of(s, &paras, &l, &q.anchor) else { continue };
        let rs = rects(&sp);
        let color = status_color(q, &p);
        if q.status != QuestionStatus::Answered || !q.seen {
            for (_, r) in rs.iter().filter(|(_, r)| visible(r)) {
                painter.rect_filled(*r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 22));
            }
        }
        underline(painter, &rs, color, false);
        let Some((page, first)) = rs.first().copied() else { continue };
        let (icon, status) = status_text(q);
        let body = match (&q.answer, q.status) {
            (Some(a), QuestionStatus::Answered) => clip(&a.text, 110),
            _ => clip(&q.text, 110),
        };
        margin.push(Margin {
            page,
            y: first.top(),
            color,
            icon: icon.into(),
            title: status.into(),
            body,
            act: Act::Open(Tab::Questions, Some(q.id.clone())),
            id: q.id.clone(),
        });
    }
    // Notes.
    let mut hovered_note = None;
    for n in &l.notes {
        let Some(sp) = span_of(s, &paras, &l, &n.anchor) else { continue };
        let rs = rects(&sp);
        underline(painter, &rs, p.note, true);
        if g.pointer.is_some_and(|pt| rs.iter().any(|(_, r)| r.expand(1.0).contains(pt))) {
            hovered_note = Some((n.clone(), rs.last().map(|(_, r)| *r)));
        }
        let Some((page, first)) = rs.first().copied() else { continue };
        let (icon, kind) = kind_label(&n.kind);
        let title = if n.title.is_empty() { kind.to_string() } else { n.title.clone() };
        margin.push(Margin {
            page,
            y: first.top(),
            color: p.note,
            icon: icon.into(),
            title,
            body: clip(&n.text, 120),
            act: Act::Open(Tab::Notes, Some(n.id.clone())),
            id: n.id.clone(),
        });
    }
    paint_margin(st, ui, g, &t, margin);
    if let Some((n, Some(at))) = hovered_note {
        hover_card(ui, &t, &p, &n, at);
    }
}

/// Margin cards beside each page (tabs inside the page's margin when there is no room), stacked
/// so they don't overlap.
fn paint_margin(st: &mut AgentUi, ui: &Ui, g: &PageGeometry, t: &Tokens, mut items: Vec<Margin>) {
    items.sort_by(|a, b| a.page.cmp(&b.page).then(a.y.total_cmp(&b.y)));
    let ctx = ui.ctx().clone();
    let mut bottom: Option<(usize, f32)> = None;
    for m in items {
        let Some(page) = g.pages().get(m.page).copied() else { continue };
        let room = g.clip.right() - page.right() - 16.0;
        let wide = room >= CARD_MIN;
        let w = if wide { room.min(CARD_MAX) } else { (page.width() * 0.13).clamp(22.0, 110.0) };
        let x = if wide { page.right() + 10.0 } else { page.right() - w - 4.0 };
        let mut y = m.y - 2.0;
        if let Some((pg, b)) = bottom
            && pg == m.page
        {
            y = y.max(b + 6.0);
        }
        let h_est = if wide { 54.0 } else { 18.0 };
        bottom = Some((m.page, y + h_est));
        if y + h_est < g.clip.top() || y > g.clip.bottom() {
            continue;
        }
        let id = egui::Id::new(("agent_margin", m.id.as_str()));
        let resp =
            egui::Area::new(id).order(egui::Order::Middle).fixed_pos(pos2(x, y)).constrain(false).sense(egui::Sense::click()).show(&ctx, |ui| {
                ui.set_clip_rect(g.clip);
                let frame = egui::Frame::NONE
                    .fill(t.panel)
                    .stroke(Stroke::new(1.0, m.color))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(if wide { egui::Margin::symmetric(8, 5) } else { egui::Margin::symmetric(5, 1) });
                frame.show(ui, |ui| {
                    ui.set_width(w - if wide { 18.0 } else { 12.0 });
                    if wide {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&m.icon).color(m.color).font(theme::semibold(12.0)));
                            ui.label(egui::RichText::new(clip(&m.title, 34)).color(m.color).font(theme::semibold(11.5)));
                        });
                        ui.label(egui::RichText::new(&m.body).color(t.text).font(theme::regular(11.0)));
                    } else {
                        ui.label(egui::RichText::new(format!("{} {}", m.icon, clip(&m.title, 12))).color(m.color).font(theme::semibold(10.5)));
                    }
                });
            });
        let card = resp.response.rect;
        let r = resp.response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            st.acts.push(m.act.clone());
        }
        bottom = Some((m.page, card.bottom()));
    }
}

/// The note under the pointer, in full, with its sources.
fn hover_card(ui: &Ui, t: &Tokens, p: &crate::Palette, n: &wordcraft_agent::layer::Note, at: Rect) {
    let (icon, kind) = kind_label(&n.kind);
    egui::Area::new(egui::Id::new("agent_note_hover"))
        .order(egui::Order::Tooltip)
        .fixed_pos(at.left_bottom() + vec2(0.0, 6.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).fill(t.menu).show(ui, |ui| {
                ui.set_max_width(340.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{icon} {kind}")).color(p.note).font(theme::semibold(11.0)));
                    ui.label(
                        egui::RichText::new(if n.author == wordcraft_agent::Actor::Agent { "· by your agent" } else { "· by a person" })
                            .color(t.text_dim)
                            .small(),
                    );
                });
                if !n.title.is_empty() {
                    ui.label(egui::RichText::new(&n.title).font(theme::semibold(13.0)));
                }
                ui.label(&n.text);
                for k in &n.links {
                    ui.label(egui::RichText::new(format!("↗ {}", k.label)).color(t.accent_text));
                    if !k.summary.is_empty() {
                        ui.label(egui::RichText::new(&k.summary).color(t.text_dim).small());
                    }
                }
                ui.label(egui::RichText::new("Click the note in the margin to ask about it").color(t.text_disabled).small());
            });
        });
}
