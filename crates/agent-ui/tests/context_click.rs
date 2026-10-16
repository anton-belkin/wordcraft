//! The Context group's mode button opens its menu with a left click (and the background session's
//! menu with a right click). Its own test binary, so the UI's globals are its own.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::{Event, Modifiers, PointerButton};
use egui_kittest::Harness;
use serde_json::json;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

fn click(h: &mut Harness<'static, WordApp>, button: PointerButton) {
    let p = wordcraft_agent_ui::mode_button_rect().expect("the mode button is drawn").center();
    h.event(Event::PointerMoved(p));
    h.event(Event::PointerButton { pos: p, button, pressed: true, modifiers: Modifiers::NONE });
    h.event(Event::PointerButton { pos: p, button, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn a_left_click_opens_the_context_menu_and_it_stays_open() {
    wordcraft_agent_ui::install();
    let mut s = Session::new(wordcraft_agent::sample::engagement_letter());
    s.run("ui.tab", &json!({"tab": "Agent"})).ok();
    let mut app = WordApp::new(s, Services::default());
    app.ui.tab = "Agent".into();
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    click(&mut h, PointerButton::Primary);
    assert!(egui::Popup::is_any_open(&h.ctx), "a left click opens the menu");
    h.run_steps(5);
    assert!(egui::Popup::is_any_open(&h.ctx), "and it stays open");
    // Its rows are there.
    use egui_kittest::kittest::Queryable;
    assert!(h.query_by_label_contains("Only this document").is_some());
    // A click on the button again closes it; a right click opens the other menu.
    click(&mut h, PointerButton::Primary);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    click(&mut h, PointerButton::Secondary);
    assert!(h.query_by_label_contains("New session").is_some(), "the right-click menu");
}
