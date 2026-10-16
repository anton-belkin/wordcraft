//! UI extension points: panels, page overlays and context-menu items contributed by crates an app
//! links in (the UI knows nothing about them). Register with [`register`] before the first frame.
//!
//! Extensions act like the rest of the UI: they read engine state from `app.session` and act
//! through `app.run(id, params)`.

use std::cell::RefCell;

use egui::{Painter, Pos2, Rect, Ui, pos2, vec2};
use wordcraft_engine::doc::Pos;
use wordcraft_engine::layout::DocLayout;

use crate::WordApp;

/// Where the document's pages and text are on screen this frame.
pub struct PageGeometry<'a> {
    layout: &'a DocLayout,
    pages: &'a [Rect],
    scale: f32,
    /// The visible part of the canvas.
    pub clip: Rect,
    /// The pointer position, if it is over the canvas.
    pub pointer: Option<Pos2>,
}

impl PageGeometry<'_> {
    /// The on-screen rectangles of the pages, in document order.
    pub fn pages(&self) -> &[Rect] {
        self.pages
    }

    /// The zoom: screen points per document point.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    fn page_scale(&self, page: usize) -> f32 {
        match (self.pages.get(page), self.layout.pages.get(page)) {
            (Some(r), Some(p)) => crate::canvas::page_screen_scale(*r, p, self.scale),
            _ => self.scale,
        }
    }

    /// The on-screen rectangles a text range covers (one per line piece), with their page.
    pub fn range_rects(&self, app: &WordApp, a: &Pos, b: &Pos) -> Vec<(usize, Rect)> {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut out = Vec::new();
        for (pi, r) in self.layout.selection_rects(&app.session.doc, a, b, app.session.page_hint) {
            if let Some(pr) = self.pages.get(pi) {
                let s = self.page_scale(pi);
                out.push((pi, Rect::from_min_size(pos2(pr.min.x + r.x * s, pr.min.y + r.y * s), vec2(r.w * s, r.h * s))));
            }
        }
        out
    }

    /// Where a caret at `pos` would be on screen (a thin rectangle the height of the line).
    pub fn caret_rect(&self, app: &WordApp, pos: &Pos) -> Option<(usize, Rect)> {
        let c = self.layout.caret_on(pos, app.session.page_hint)?;
        let pr = self.pages.get(c.page)?;
        let s = self.page_scale(c.page);
        Some((c.page, Rect::from_min_size(pos2(pr.min.x + c.x * s, pr.min.y + c.top * s), vec2(1.0, c.height * s))))
    }
}

/// A ribbon tab an extension adds, after the built-in ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RibbonTab {
    pub name: &'static str,
    /// Its keytip after Alt (empty for none); must not clash with the built-in tabs' letters.
    pub keytip: &'static str,
}

/// A UI extension. Every method has a default that does nothing.
pub trait UiExtension {
    /// Once per frame, before anything is drawn (shortcuts, polling, acting on clicks).
    fn frame(&mut self, _app: &mut WordApp, _ctx: &egui::Context) {}
    /// A side panel; show one with `egui::Panel::right(..).show(ui, ..)` or nothing.
    fn side_panel(&mut self, _app: &mut WordApp, _ui: &mut Ui) {}
    /// Paint over the pages (after the text and Find's highlights, before the selection).
    fn paint_pages(&mut self, _app: &WordApp, _ui: &Ui, _painter: &Painter, _pages: &PageGeometry) {}
    /// Items appended to the text context menu.
    fn context_menu(&mut self, _app: &mut WordApp, _ui: &mut Ui) {}
    /// Ribbon tabs this extension adds.
    fn ribbon_tabs(&self) -> Vec<RibbonTab> {
        Vec::new()
    }
    /// Draws the groups of ribbon tab `name` (with `crate::widgets::group` and friends, like the
    /// built-in tabs) if it is one of this extension's; returns whether it was.
    fn ribbon_tab(&mut self, _app: &mut WordApp, _ui: &mut Ui, _name: &str) -> bool {
        false
    }
}

thread_local! {
    static EXTENSIONS: RefCell<Vec<Box<dyn UiExtension>>> = RefCell::new(Vec::new());
}

/// Adds a UI extension (on the UI thread).
pub fn register(ext: Box<dyn UiExtension>) {
    EXTENSIONS.with(|e| e.borrow_mut().push(ext));
}

/// Runs `f` on every extension. The list is taken out while they run, so an extension may use
/// the whole app (and even register another extension).
fn each(mut f: impl FnMut(&mut dyn UiExtension)) {
    let mut list = EXTENSIONS.with(|e| std::mem::take(&mut *e.borrow_mut()));
    if list.is_empty() {
        return;
    }
    for ext in &mut list {
        f(ext.as_mut());
    }
    EXTENSIONS.with(|e| {
        let mut slot = e.borrow_mut();
        list.append(&mut slot);
        *slot = list;
    });
}

pub(crate) fn frame(app: &mut WordApp, ctx: &egui::Context) {
    each(|x| x.frame(app, ctx));
}

pub(crate) fn side_panels(app: &mut WordApp, ui: &mut Ui) {
    each(|x| x.side_panel(app, ui));
}

pub(crate) fn paint_pages(app: &WordApp, ui: &Ui, painter: &Painter, layout: &DocLayout, pages: &[Rect], scale: f32) {
    let clip = ui.clip_rect();
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| clip.contains(*p));
    let g = PageGeometry { layout, pages, scale, clip, pointer };
    each(|x| x.paint_pages(app, ui, painter, &g));
}

pub(crate) fn context_menu(app: &mut WordApp, ui: &mut Ui) {
    each(|x| x.context_menu(app, ui));
}

pub(crate) fn ribbon_tabs() -> Vec<RibbonTab> {
    let mut v = Vec::new();
    each(|x| v.extend(x.ribbon_tabs()));
    v
}

/// The keytip of an extension's tab.
pub(crate) fn tab_letter(tab: &str) -> Option<&'static str> {
    ribbon_tabs().into_iter().find(|t| t.name == tab).map(|t| t.keytip)
}

/// Whether `tab` is an extension's tab.
pub(crate) fn is_tab(tab: &str) -> bool {
    ribbon_tabs().iter().any(|t| t.name == tab)
}

/// Draws the active tab's groups (an extension's tab).
pub(crate) fn ribbon_tab(app: &mut WordApp, ui: &mut Ui) {
    let name = app.ui.tab.clone();
    let mut done = false;
    each(|x| {
        if !done {
            done = x.ribbon_tab(app, ui, &name);
        }
    });
}
