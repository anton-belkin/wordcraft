//! Push to talk in the running UI (egui_kittest): hold ⌥ and a recording starts, release and the
//! question is asked (and keytips stay closed); ⌥ with another key is a shortcut, not a question.
//! A stand-in microphone, no transcription: this is about the key. Its own test binary, so the
//! UI's globals are its own.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use serde_json::json;
use wordcraft_agent::layer::QuestionStatus;
use wordcraft_engine::Session;
use wordcraft_ui_egui::keytips::Phase;
use wordcraft_ui_egui::{Services, WordApp};

fn harness() -> Harness<'static, WordApp> {
    wordcraft_agent_ui::install();
    let dir = std::env::temp_dir().join(format!("wordcraft-voice-key-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    wordcraft_agent_ui::voice::set_settings_path(Some(dir.join("voice.json")));
    // A second and a half of something above silence.
    wordcraft_agent_ui::voice::use_fake_microphone(Some((0..24_000).map(|i| ((i as f32) * 0.05).sin() * 0.1).collect()));
    let mut s = Session::new(wordcraft_agent::sample::engagement_letter());
    s.run("agent.voiceModel", &json!({"model": "small"})).unwrap();
    s.run("select.text", &json!({"text": "payable within 30 days"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        WordApp::new(s, Services::default()),
    );
    h.run_steps(4);
    h
}

/// macOS: the modifiers change first, ⌥'s own key event comes a frame later.
fn press_option(h: &mut Harness<'static, WordApp>) {
    h.event(Event::ModifiersChanged(Modifiers::ALT));
    h.run_steps(1);
    h.event(Event::Key { key: Key::AltLeft, pressed: true, modifiers: Modifiers::ALT, repeat: false, physical_key: None });
    h.run_steps(1);
}

fn release_option(h: &mut Harness<'static, WordApp>) {
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.event(Event::Key { key: Key::AltLeft, pressed: false, modifiers: Modifiers::NONE, repeat: false, physical_key: None });
    h.run_steps(3);
}

fn recording() -> bool {
    let mut s = Session::new(wordcraft_doc::Document::new());
    s.run("agent.voiceStatus", &json!({})).unwrap()["recording"] == json!(true)
}

fn questions(h: &Harness<'static, WordApp>) -> Vec<wordcraft_agent::layer::Question> {
    wordcraft_agent::layer(&h.state().session).questions.clone()
}

#[test]
fn holding_option_records_and_releasing_asks() {
    let mut h = harness();
    // ⌥ alone, held: nothing at first (a tap isn't a question), recording after a moment.
    press_option(&mut h);
    assert!(!recording());
    h.run_steps(20);
    assert!(recording(), "holding ⌥ should start recording");
    // Pointing while speaking: the selection moves to another passage.
    h.state_mut().session.run("select.text", &json!({"text": "ten years"})).unwrap();
    h.run_steps(3);
    // Release: the question is asked at once, on the first selection with the passage pointed at.
    release_option(&mut h);
    assert!(!recording(), "releasing ⌥ should stop recording");
    let qs = questions(&h);
    assert_eq!(qs.len(), 1, "{qs:?}");
    assert!(qs[0].spoken);
    assert_eq!(qs[0].refs.len(), 1);
    // The stand-in microphone transcribes nothing: a failed flag, no dialog.
    assert_eq!(qs[0].status, QuestionStatus::Failed);
    // Releasing ⌥ after a question doesn't open keytips.
    assert_eq!(h.state().ui.keytips, Phase::Off);
    // Its topic has the passage asked on and the one pointed at.
    let topics = wordcraft_agent::topics::resolve(&h.state().session);
    assert_eq!(topics.first().map(|t| t.places.len()), Some(2));

    // ⌥ with another key is a shortcut: no recording, no question.
    press_option(&mut h);
    h.event(Event::Key { key: Key::E, pressed: true, modifiers: Modifiers::ALT, repeat: false, physical_key: None });
    h.run_steps(25);
    assert!(!recording());
    release_option(&mut h);
    assert_eq!(questions(&h).len(), 1);
}
