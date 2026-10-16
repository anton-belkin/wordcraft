//! Voice questions: hold ⌥ (Alt) or click the microphone on the Agent tab, speak, click or select
//! the passages you mean while speaking; release, and the question goes on in the background.
//!
//! The question is put on the document at once (a topic, one place per passage pointed at, so
//! they move with edits; "transcribing", not with the agent yet); [`craft_voice`] transcribes it
//! on a worker thread and the text, with a marker `⟦n⟧` where each passage was pointed at, goes to
//! the agent's queue (`agent.spoken`). No preview, no confirmation. A failed transcription is a
//! flag with Retry and Dismiss, never a dialog. Audio stays in memory until its text is there (or
//! the reader dismisses it), and is never written anywhere.
//!
//! The model is the reader's choice on the Agent tab (off until they pick one), an app setting
//! (`voice.json` in the agent's settings folder), never the document's. The same as GridCraft's.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use craft_voice::{Audio, Event, Model, Pointer, Recorder, Transcriber};
use egui::{Align2, Color32, Rect, Stroke, pos2, vec2};
use serde_json::{Value as Json, json};
use wordcraft_agent::anchor;
use wordcraft_doc::{Pos, StoryRef};
use wordcraft_engine::{CmdError, CmdResult, CommandSpec, Selection, Session};
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::ext::PageGeometry;
use wordcraft_ui_egui::theme::{self, Tokens};

/// How long ⌥ is held alone before recording starts (a tap or a shortcut is not a question).
const HOLD: f64 = 0.2;
/// Shorter recordings with nothing pointed at are taken for an accidental press.
const MIN_SECONDS: f32 = 0.4;
/// A loaded model is let go after this long without a question (it holds 0.6–1.3 GB).
const IDLE_UNLOAD: f64 = 15.0 * 60.0;

/// The key, as people see it.
pub(crate) const KEY: &str = if cfg!(target_os = "macos") { "⌥ Option" } else { "Alt" };

/// Where the model stands, for the ribbon.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Status {
    /// No model chosen.
    #[default]
    Off,
    /// Chosen, downloaded, not loaded (loads with the first question).
    Idle,
    Downloading(u64, u64),
    Loading,
    Ready,
    Failed(String),
}

impl Status {
    fn id(&self) -> &'static str {
        match self {
            Status::Off => "off",
            Status::Idle => "idle",
            Status::Downloading(..) => "downloading",
            Status::Loading => "loading",
            Status::Ready => "ready",
            Status::Failed(_) => "failed",
        }
    }
}

/// One passage pointed at: its range in the body.
pub(crate) type Anchor = (Pos, Pos);

struct Recording {
    /// None in a preview (offscreen snapshots) or with a stand-in microphone.
    recorder: Option<Recorder>,
    fake: Option<Vec<f32>>,
    preview_secs: f32,
    doc: u64,
    /// The selection when it started: what the question is asked on.
    main: Selection,
    pointing: Pointing,
    by_key: bool,
}

/// Pointing while recording, frame by frame: every click in the text and every other selection
/// change becomes a pointer at the moment it began.
#[derive(Debug, Default)]
pub(crate) struct Pointing {
    pub pointers: Vec<Pointer<Anchor>>,
    /// The selection last seen.
    last: Option<Anchor>,
    /// When a click in the text began (seconds into the recording), and whether a frame has
    /// passed since the button came up.
    press: Option<f32>,
    settled: bool,
}

/// Pointing at the same place again within this long adds no marker (a double click).
const SAME_PLACE: f32 = 1.0;

impl Pointing {
    pub(crate) fn new(selection: Anchor) -> Pointing {
        Pointing { last: Some(selection), ..Pointing::default() }
    }

    /// One frame. Extension frames run before the canvas handles this frame's input, so a click's
    /// selection is read a frame after the button comes up, never the one before it.
    pub(crate) fn observe(&mut self, now: f32, pressed_in_text: bool, down: bool, selection: Anchor, place: impl FnOnce() -> Option<Anchor>) {
        if pressed_in_text {
            self.press = Some(now);
            self.settled = false;
        }
        if down {
            return;
        }
        if let Some(t) = self.press {
            if !self.settled {
                self.settled = true;
                return;
            }
            if let Some(a) = place() {
                self.add(t, a);
            }
            self.press = None;
            self.last = Some(selection);
            return;
        }
        // The selection moved some other way (keys, Find).
        if self.last.as_ref() != Some(&selection) {
            if let Some(a) = place() {
                self.add(now, a);
            }
            self.last = Some(selection);
        }
    }

    fn add(&mut self, at: f32, anchor: Anchor) {
        if self.pointers.iter().any(|p| p.anchor == anchor && (p.at - at).abs() < SAME_PLACE) {
            return;
        }
        self.pointers.push(Pointer { at, anchor });
    }
}

/// What a click or a selection points at: the selected passage, or at a caret the word there
/// (the paragraph when the caret isn't in a word).
pub(crate) fn pointed_at(s: &Session) -> Option<Anchor> {
    let (a, b) = s.sel.ordered();
    if a.story != StoryRef::Body || b.story != StoryRef::Body {
        return None;
    }
    if a != b {
        return Some((a, b));
    }
    let text = anchor::para_text(&s.doc, &a.path).unwrap_or("");
    let word = |c: char| c.is_alphanumeric() || matches!(c, '-' | '\'' | '’' | '.' | ',' | '%') && c != wordcraft_doc::para::OBJ;
    let at = a.off.min(text.len());
    let start = text.get(..at).map_or(at, |t| t.char_indices().rev().take_while(|(_, c)| word(*c)).last().map_or(at, |(i, _)| i));
    let end = text.get(at..).map_or(at, |t| at + t.char_indices().take_while(|(_, c)| word(*c)).last().map_or(0, |(i, c)| i + c.len_utf8()));
    // Trailing punctuation isn't the word's.
    let end = text.get(start..end).map_or(end, |w| start + w.trim_end_matches(['.', ',']).len());
    if start < end {
        Some((Pos { off: start, ..a.clone() }, Pos { off: end, ..a }))
    } else {
        Some((Pos { off: 0, ..a.clone() }, Pos { off: text.len(), ..a }))
    }
}

/// A question waiting for its text.
struct Job {
    doc: u64,
    question: String,
    pointers: Vec<Pointer<Anchor>>,
    /// Kept for a retry until the text is there.
    audio: Audio,
    /// The id of its transcription now.
    ticket: u64,
    failed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Key {
    #[default]
    Idle,
    Armed(f64),
    /// ⌥ went into a shortcut: wait for it to be released.
    Chord,
    Recording,
}

#[derive(Default)]
pub(crate) struct Voice {
    choice: Option<Model>,
    settings_read: bool,
    transcriber: Option<Transcriber>,
    status: Status,
    rec: Option<Recording>,
    jobs: Vec<Job>,
    next_ticket: u64,
    key: Key,
    last_used: f64,
}

/// A model chosen through `agent.voiceModel` (control channel, MCP), taken by the UI next frame.
static REQUEST: Mutex<Option<Option<Model>>> = Mutex::new(None);
/// What the ribbon shows, for `agent.voiceStatus`.
static SNAPSHOT: Mutex<Option<Json>> = Mutex::new(None);
/// The settings file, when a test moves it.
static SETTINGS: Mutex<Option<PathBuf>> = Mutex::new(None);

/// What an offscreen snapshot shows instead of a live microphone and download.
#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct Preview {
    /// Recording for this many seconds, these passages pointed at (quotes found in the document).
    pub recording: Option<(f32, Vec<String>)>,
    /// A download this far along (bytes, total).
    pub downloading: Option<(u64, u64)>,
}

static PREVIEW: Mutex<Option<Preview>> = Mutex::new(None);

/// Shows a recording or a download without a microphone or a network (snapshots).
#[doc(hidden)]
pub fn preview(p: Preview) {
    if let Ok(mut g) = PREVIEW.lock() {
        *g = Some(p);
    }
}

/// A stand-in microphone (tests): recordings return these samples, and nothing is transcribed.
static FAKE_MIC: Mutex<Option<Vec<f32>>> = Mutex::new(None);

/// Records from a stand-in instead of the microphone, and transcribes nothing (tests).
#[doc(hidden)]
pub fn use_fake_microphone(samples: Option<Vec<f32>>) {
    if let Ok(mut g) = FAKE_MIC.lock() {
        *g = samples;
    }
}

fn fake_mic() -> Option<Vec<f32>> {
    FAKE_MIC.lock().ok().and_then(|g| g.clone())
}

/// Moves the settings file (tests).
pub fn set_settings_path(p: Option<PathBuf>) {
    if let Ok(mut g) = SETTINGS.lock() {
        *g = p;
    }
}

fn settings_path() -> Option<PathBuf> {
    if let Some(p) = SETTINGS.lock().ok().and_then(|g| g.clone()) {
        return Some(p);
    }
    if let Some(p) = std::env::var_os("WORDCRAFT_VOICE_SETTINGS").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    wordcraft_agent::runner::config().data_dir.map(|d| d.join("voice.json"))
}

fn read_choice() -> Option<Model> {
    let text = std::fs::read_to_string(settings_path()?).ok()?;
    let v: Json = serde_json::from_str(&text).ok()?;
    Model::from_id(v.get("model")?.as_str()?)
}

fn write_choice(m: Option<Model>) -> Result<(), String> {
    let Some(p) = settings_path() else { return Err("no settings folder".into()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, json!({"model": m.map_or("off", Model::id)}).to_string()).map_err(|e| e.to_string())
}

fn model_dir() -> Option<PathBuf> {
    craft_voice::default_dir()
}

// ---------------------------------------------------------------- commands

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("agent.voiceModel", "Voice Questions Model", "Review › Agent", voice_model)
            .params("{model: off|small|turbo} — the speech model for voice questions (an app setting; downloaded on first use)")
            .pure(),
        CommandSpec::new("agent.voiceStatus", "Voice Questions Status", "Review › Agent", voice_status)
            .params("{} → {model, status, done?, total?, error?, recording, transcribing}")
            .pure(),
    ]
}

fn voice_model(_s: &mut Session, p: &Json) -> CmdResult {
    let id = p.get("model").and_then(Json::as_str).ok_or_else(|| CmdError::Params("missing `model` (off, small or turbo)".into()))?;
    let m = match id {
        "off" => None,
        id => Some(Model::from_id(id).ok_or_else(|| CmdError::Params("`model` is off, small or turbo".into()))?),
    };
    if let Ok(mut r) = REQUEST.lock() {
        *r = Some(m);
    }
    // Without a settings folder (tests, snapshots) the choice holds for this run only.
    let kept = match settings_path() {
        Some(_) => {
            write_choice(m).map_err(|e| CmdError::Failed(format!("couldn't keep the setting: {e}")))?;
            true
        }
        None => false,
    };
    Ok(json!({"model": m.map_or("off", Model::id), "kept": kept}))
}

fn voice_status(_s: &mut Session, _p: &Json) -> CmdResult {
    Ok(SNAPSHOT.lock().ok().and_then(|g| g.clone()).unwrap_or_else(|| json!({"model": "off", "status": "off"})))
}

// ---------------------------------------------------------------- the frame

impl Voice {
    pub(crate) fn choice(&self) -> Option<Model> {
        self.choice
    }

    pub(crate) fn status(&self) -> &Status {
        &self.status
    }

    pub(crate) fn recording(&self) -> bool {
        self.rec.is_some()
    }

    /// Seconds recorded and the input level, while recording.
    pub(crate) fn meter(&self) -> Option<(f32, f32)> {
        self.rec.as_ref().map(|r| r.recorder.as_ref().map_or((r.preview_secs, 0.35), |m| (m.position(), m.level())))
    }

    /// The passages pointed at so far, numbered (while recording).
    pub(crate) fn pointed(&self) -> Vec<(u32, Anchor)> {
        let Some(r) = &self.rec else { return vec![] };
        numbered(&r.pointing.pointers)
    }

    /// The reader picked a model (or off) on the ribbon.
    pub(crate) fn choose(&mut self, app: &mut WordApp, ctx: &egui::Context, m: Option<Model>) {
        if let Err(e) = write_choice(m) {
            app.status(format!("Voice: couldn't keep the setting ({e})"));
        }
        self.apply(app, ctx, m);
    }

    fn apply(&mut self, app: &mut WordApp, ctx: &egui::Context, m: Option<Model>) {
        self.settings_read = true;
        if self.choice == m {
            return;
        }
        self.choice = m;
        self.transcriber = None;
        self.status = match m {
            None => Status::Off,
            Some(_) => Status::Idle,
        };
        // Downloaded on first use: picking a model that isn't here downloads it now, with its
        // progress on the ribbon. One that is here loads with the first question.
        if let Some(m) = m
            && !model_dir().is_some_and(|d| m.is_ready(&d))
        {
            self.start(ctx);
        }
        // Questions waiting for text go to the new model (or fail when voice is off).
        let waiting: Vec<usize> = (0..self.jobs.len()).filter(|&i| self.jobs.get(i).is_some_and(|j| !j.failed)).collect();
        for i in waiting {
            self.resend(app, ctx, i);
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (Some(m), Some(dir)) = (self.choice, model_dir()) else { return };
        if self.transcriber.is_some() || fake_mic().is_some() {
            return;
        }
        let c = ctx.clone();
        let languages = std::env::var("WORDCRAFT_VOICE_LANGUAGES")
            .map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();
        self.transcriber = Some(Transcriber::start(m, dir, craft_voice::Options { languages }, move || c.request_repaint()));
        self.status = if m.is_ready(&model_dir().unwrap_or_default()) { Status::Loading } else { Status::Downloading(0, m.download_size()) };
    }

    /// Every frame: the key, the pointing, the transcriber's news.
    pub(crate) fn frame(&mut self, app: &mut WordApp, ctx: &egui::Context) {
        if !self.settings_read {
            self.settings_read = true;
            let m = read_choice();
            self.apply(app, ctx, m);
        }
        if let Some(m) = REQUEST.lock().ok().and_then(|mut r| r.take()) {
            self.apply(app, ctx, m);
        }
        if let Some(p) = PREVIEW.lock().ok().and_then(|mut g| g.take()) {
            self.show_preview(app, p);
        }
        let now = ctx.input(|i| i.time);
        self.key(app, ctx, now);
        self.pointing(app, ctx);
        self.news(app, now);
        if self.jobs.is_empty() && self.rec.is_none() && self.transcriber.is_some() && now - self.last_used > IDLE_UNLOAD {
            // Let the model go; the next question loads it again.
            self.transcriber = None;
            self.status = if self.choice.is_some() { Status::Idle } else { Status::Off };
        }
        if let Ok(mut g) = SNAPSHOT.lock() {
            let mut v = json!({
                "model": self.choice.map_or("off", Model::id),
                "status": self.status.id(),
                "recording": self.rec.is_some(),
                "transcribing": self.jobs.iter().filter(|j| !j.failed).count(),
                "failed": self.jobs.iter().filter(|j| j.failed).count(),
            });
            match &self.status {
                Status::Downloading(done, total) => {
                    v["done"] = json!(done);
                    v["total"] = json!(total);
                }
                Status::Failed(e) => v["error"] = json!(e),
                _ => {}
            }
            *g = Some(v);
        }
        if self.rec.is_some() {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
    }

    /// Push to talk: ⌥ held alone starts recording, its release sends; Esc cancels.
    fn key(&mut self, app: &mut WordApp, ctx: &egui::Context, now: f64) {
        let (mods, keys, esc) = ctx.input(|i| {
            // Another key with ⌥ makes it a shortcut. Modifier keys themselves don't count: egui
            // reports ⌥ as a key too, sometimes a frame after the modifiers changed.
            let modifier = |k: &egui::Key| {
                use egui::Key::*;
                matches!(k, AltLeft | AltRight | ShiftLeft | ShiftRight | ControlLeft | ControlRight | SuperLeft | SuperRight)
            };
            let keys = i.events.iter().any(|e| match e {
                egui::Event::Key { key, pressed: true, .. } => !modifier(key),
                egui::Event::Text(_) => true,
                _ => false,
            });
            let esc = i.key_pressed(egui::Key::Escape);
            (i.modifiers, keys, esc)
        });
        let alone = mods.alt && !mods.shift && !mods.command && !mods.ctrl && !mods.mac_cmd;
        if esc && self.rec.is_some() {
            self.cancel();
            self.key = if mods.alt { Key::Chord } else { Key::Idle };
            return;
        }
        // Not while typing in a text box of the app (the document's own canvas is fine: ⌥ alone
        // types nothing there).
        let typing = ctx.text_edit_focused() || app.dialog.is_some() || app.ui.backstage;
        self.key = match self.key {
            Key::Idle if alone && !typing && self.choice.is_some() && self.rec.is_none() && app.has_document() => {
                ctx.request_repaint_after(Duration::from_secs_f64(HOLD + 0.02));
                Key::Armed(now)
            }
            Key::Idle => Key::Idle,
            Key::Armed(_) if !mods.alt => Key::Idle,
            Key::Armed(_) if keys || !alone => Key::Chord,
            Key::Armed(t) if now - t >= HOLD => {
                if self.begin(app, ctx, true) {
                    Key::Recording
                } else {
                    Key::Chord
                }
            }
            Key::Armed(t) => {
                ctx.request_repaint_after(Duration::from_secs_f64((HOLD - (now - t)).max(0.0) + 0.02));
                Key::Armed(t)
            }
            Key::Chord if mods.alt => Key::Chord,
            Key::Chord => Key::Idle,
            Key::Recording if self.rec.is_none() => {
                if mods.alt {
                    Key::Chord
                } else {
                    Key::Idle
                }
            }
            Key::Recording if !mods.alt => {
                if self.rec.as_ref().is_some_and(|r| r.by_key) {
                    self.finish(app, ctx);
                }
                Key::Idle
            }
            Key::Recording => Key::Recording,
        };
        // Releasing a held ⌥ after a question is not a keytips press (this runs before keytips).
        if self.key == Key::Recording || (self.key == Key::Idle && mods.alt) {
            app.ui.alt_chord_used = true;
        }
    }

    fn show_preview(&mut self, app: &WordApp, p: Preview) {
        if let Some((done, total)) = p.downloading {
            self.status = Status::Downloading(done, total);
        }
        let Some((secs, quotes)) = p.recording else { return };
        let s = &app.session;
        let found: Vec<Anchor> = quotes
            .iter()
            .filter_map(|q| {
                let view = wordcraft_agent::mask::View::new(&s.doc, &Default::default(), &[]);
                view.find(None, Some(q), 0).ok().map(|sp| (sp.start_pos(), sp.end_pos()))
            })
            .collect();
        let n = found.len().max(1) as f32;
        let main = s.sel.clone();
        let (a, b) = main.ordered();
        self.rec = Some(Recording {
            recorder: None,
            fake: None,
            preview_secs: secs,
            doc: s.document_id(),
            main,
            pointing: Pointing {
                pointers: found.into_iter().enumerate().map(|(i, a)| Pointer { at: secs * (i as f32 + 0.5) / n, anchor: a }).collect(),
                ..Pointing::new((a, b))
            },
            by_key: true,
        });
    }

    /// The microphone button: start, or stop and send.
    pub(crate) fn toggle(&mut self, app: &mut WordApp, ctx: &egui::Context) {
        if self.rec.is_some() {
            self.finish(app, ctx);
        } else {
            self.begin(app, ctx, false);
        }
    }

    fn begin(&mut self, app: &mut WordApp, ctx: &egui::Context, by_key: bool) -> bool {
        if !app.has_document() {
            return false;
        }
        if self.choice.is_none() {
            app.status("Voice questions: choose a speech model on the Agent tab first");
            return false;
        }
        let s = &app.session;
        let main = s.sel.clone();
        let doc = s.document_id();
        let fake = fake_mic();
        let started = if fake.is_some() { Ok(None) } else { Recorder::start().map(Some) };
        match started {
            Ok(recorder) => {
                let (a, b) = main.ordered();
                self.rec = Some(Recording { recorder, fake, preview_secs: 0.0, doc, main, pointing: Pointing::new((a, b)), by_key });
                // Load the model while the reader speaks.
                self.start(ctx);
                self.last_used = ctx.input(|i| i.time);
                true
            }
            Err(e) => {
                app.status(format!("Voice: {e}"));
                false
            }
        }
    }

    fn cancel(&mut self) {
        // Dropping the recorder drops the audio.
        self.rec = None;
    }

    /// While recording: every click in the text and every selection change is a pointer, at the
    /// moment it began.
    fn pointing(&mut self, app: &WordApp, ctx: &egui::Context) {
        let Some(r) = self.rec.as_mut() else { return };
        let s = &app.session;
        if s.document_id() != r.doc {
            return;
        }
        let at = r.recorder.as_ref().map_or(r.preview_secs, Recorder::position);
        let (pressed, down, pos) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos()));
        let in_text = pressed && matches!((app.canvas.canvas_rect, pos), (Some(c), Some(p)) if c.contains(p));
        let sel = s.sel.ordered();
        r.pointing.observe(at, in_text, down, sel, || pointed_at(s));
    }

    /// Key up: the question goes on the document now, its text follows.
    fn finish(&mut self, app: &mut WordApp, ctx: &egui::Context) {
        let Some(r) = self.rec.take() else { return };
        let (doc, main, pointers) = (r.doc, r.main, r.pointing.pointers);
        let audio = match (r.recorder, r.fake) {
            (Some(recorder), _) => recorder.stop(),
            (None, Some(samples)) => Ok(Audio { samples }),
            // A preview: nothing was recorded.
            (None, None) => return,
        };
        let audio = match audio {
            Ok(a) => a,
            Err(e) => {
                app.status(format!("Voice: {e}"));
                return;
            }
        };
        if audio.seconds() < MIN_SECONDS && pointers.is_empty() {
            return;
        }
        self.submit(app, ctx, doc, main, pointers, audio);
    }

    /// A recording to ask about: on the document now (transcribing), to the transcriber.
    fn submit(&mut self, app: &mut WordApp, ctx: &egui::Context, doc: u64, main: Selection, pointers: Vec<Pointer<Anchor>>, audio: Audio) {
        let s = &app.session;
        let refs: Vec<Json> = numbered(&pointers)
            .into_iter()
            .filter_map(|(n, (a, b))| {
                let sp = crate::range_span(s, &a, &b)?;
                let mut p = crate::span_params(s, &sp);
                p["n"] = json!(n);
                Some(p)
            })
            .collect();
        let params = json!({"transcribing": true, "spoken": true, "refs": refs});
        // Asked on the selection it started with (the reader may have clicked elsewhere since).
        let now = app.session.sel.clone();
        app.session.sel = Selection { anchor: app.session.doc.clamp(&main.anchor), focus: app.session.doc.clamp(&main.focus) };
        let asked = on_doc(app, doc, "agent.ask", params);
        app.session.sel = Selection { anchor: app.session.doc.clamp(&now.anchor), focus: app.session.doc.clamp(&now.focus) };
        let question = match asked {
            Ok(v) => v.get("id").and_then(Json::as_str).unwrap_or_default().to_string(),
            Err(e) => {
                app.status(format!("Voice: {e}"));
                return;
            }
        };
        self.jobs.push(Job { doc, question, pointers, audio, ticket: 0, failed: false });
        let i = self.jobs.len() - 1;
        self.resend(app, ctx, i);
    }

    /// (Re)sends a job's audio to the transcriber.
    fn resend(&mut self, app: &mut WordApp, ctx: &egui::Context, i: usize) {
        self.start(ctx);
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        let Some(job) = self.jobs.get_mut(i) else { return };
        let sent = self.transcriber.as_ref().is_some_and(|t| t.transcribe(ticket, job.audio.clone()));
        job.ticket = ticket;
        job.failed = !sent;
        let (doc, q) = (job.doc, job.question.clone());
        let params =
            if sent { json!({"id": q}) } else { json!({"id": q, "error": "Voice questions are off: choose a speech model on the Agent tab"}) };
        let _ = on_doc(app, doc, "agent.spoken", params);
        self.last_used = ctx.input(|i| i.time);
    }

    /// Retry on a failed spoken question.
    pub(crate) fn retry(&mut self, app: &mut WordApp, ctx: &egui::Context, question: &str) {
        match self.jobs.iter().position(|j| j.question == question) {
            Some(i) => self.resend(app, ctx, i),
            // The audio is gone (the app was restarted): nothing to retry.
            None => app.status("This recording is no longer here: ask again"),
        }
    }

    /// Dismiss on a spoken question: it goes, with its recording.
    pub(crate) fn dismiss(&mut self, app: &mut WordApp, question: &str) {
        self.jobs.retain(|j| j.question != question);
        let doc = app.session.document_id();
        if let Err(e) = on_doc(app, doc, "agent.removeQuestion", json!({"id": question})) {
            app.status(e);
        }
    }

    fn news(&mut self, app: &mut WordApp, now: f64) {
        let Some(t) = &self.transcriber else { return };
        for e in t.poll() {
            match e {
                Event::Downloading { done, total, .. } => self.status = Status::Downloading(done, total),
                Event::Loading { .. } => self.status = Status::Loading,
                Event::Ready { .. } => self.status = Status::Ready,
                Event::Failed { id: None, error } => self.status = Status::Failed(error),
                Event::Failed { id: Some(ticket), error } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.ticket == ticket) {
                        j.failed = true;
                        let (doc, q) = (j.doc, j.question.clone());
                        let _ = on_doc(app, doc, "agent.spoken", json!({"id": q, "error": error}));
                    }
                }
                Event::Text { id: ticket, transcript, secs } => {
                    self.last_used = now;
                    let Some(i) = self.jobs.iter().position(|j| j.ticket == ticket) else { continue };
                    let Some(j) = self.jobs.get_mut(i) else { continue };
                    let (doc, q) = (j.doc, j.question.clone());
                    if transcript.is_empty() {
                        j.failed = true;
                        let _ = on_doc(app, doc, "agent.spoken", json!({"id": q, "error": "Nothing was heard"}));
                        continue;
                    }
                    let text = craft_voice::splice(&transcript, &j.pointers).text;
                    // When: enough to see why a marker landed where it did (text and times only,
                    // never audio).
                    let segs: Vec<String> = transcript.segments.iter().map(|g| format!("{:.2}–{:.2} {}", g.start, g.end, g.text)).collect();
                    let ptrs: Vec<String> = j
                        .pointers
                        .iter()
                        .map(|p| format!("{:.2} §{}@{}", p.at, p.anchor.0.path.0.first().copied().unwrap_or(0), p.anchor.0.off))
                        .collect();
                    log::info!("voice {q}: {secs:.2} s, {} · segments [{}] · pointers [{}]", transcript.language, segs.join(" | "), ptrs.join(", "));
                    // With the agent now (or dismissed, or its document closed): the audio goes.
                    let _ = on_doc(app, doc, "agent.spoken", json!({"id": q, "text": text}));
                    self.jobs.remove(i);
                }
            }
        }
        // Questions whose document was closed: let their audio go.
        let open = app.session.document_id();
        self.jobs.retain(|j| j.doc == open);
    }
}

/// The references in the order they were pointed at, the same place reusing its number: the
/// numbering `craft_voice::splice` gives the markers.
fn numbered(pointers: &[Pointer<Anchor>]) -> Vec<(u32, Anchor)> {
    let mut order: Vec<&Pointer<Anchor>> = pointers.iter().collect();
    order.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut out: Vec<(u32, Anchor)> = vec![];
    for p in order {
        if !out.iter().any(|(_, a)| *a == p.anchor) {
            let n = u32::try_from(out.len()).unwrap_or(u32::MAX).saturating_add(1);
            out.push((n, p.anchor.clone()));
        }
    }
    out
}

/// Runs a command on the document the question was asked in (a window holds one).
pub(crate) fn on_doc(app: &mut WordApp, doc: u64, id: &str, params: Json) -> Result<Json, String> {
    if app.session.document_id() != doc {
        return Err("the document was closed".into());
    }
    app.run(id, params)
}

// ---------------------------------------------------------------- drawing

/// While recording: a pill at the bottom of the page area (time, level, how to send or cancel).
pub(crate) fn indicator(v: &Voice, app: &WordApp, ctx: &egui::Context) {
    let (Some((secs, level)), Some(area)) = (v.meter(), app.canvas.canvas_rect) else { return };
    let t = Tokens::get(ctx);
    let red = Color32::from_rgb(0xD9, 0x30, 0x25);
    let by_key = v.rec.as_ref().is_some_and(|r| r.by_key);
    let key = if cfg!(target_os = "macos") { "Option" } else { "Alt" };
    let hint = if by_key { format!("release {key} to send · Esc cancels") } else { "click the microphone to send · Esc cancels".into() };
    let n = v.pointed().len();
    let text = if n > 0 {
        format!("Listening {secs:.0}s · {n} pointed · {hint}")
    } else {
        format!("Listening {secs:.0}s · click the words you mean · {hint}")
    };
    let pos = pos2(area.center().x, area.bottom() - 18.0);
    egui::Area::new(egui::Id::new("agent_voice_indicator"))
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE.fill(t.menu).stroke(Stroke::new(1.0, red)).corner_radius(14.0).inner_margin(egui::Margin::symmetric(10, 5)).show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(64.0, 16.0), egui::Sense::hover());
                        let pulse = 0.6 + 0.4 * ((ui.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5) as f32;
                        ui.painter().circle_filled(pos2(r.left() + 6.0, r.center().y), 5.0, red.gamma_multiply(pulse));
                        // A level meter: five bars.
                        let lv = (level * 8.0).clamp(0.0, 1.0);
                        for k in 0..5 {
                            let h = 3.0 + 10.0 * (lv * (1.0 - (k as f32 - 2.0).abs() * 0.18)).max(0.05);
                            let x = r.left() + 18.0 + k as f32 * 8.0;
                            let bar = Rect::from_center_size(pos2(x, r.center().y), vec2(4.0, h));
                            ui.painter().rect_filled(bar, 2.0, if lv > 0.05 { red } else { t.text_dim });
                        }
                        ui.label(egui::RichText::new(text).font(theme::regular(12.0)).color(t.text));
                    });
                },
            );
        });
}

/// While recording: a numbered bubble on each passage pointed at.
pub(crate) fn bubbles(v: &Voice, app: &WordApp, painter: &egui::Painter, g: &PageGeometry) {
    if v.rec.as_ref().is_none_or(|r| r.doc != app.session.document_id()) {
        return;
    }
    let red = Color32::from_rgb(0xD9, 0x30, 0x25);
    for (n, (a, b)) in v.pointed() {
        let rects = g.range_rects(app, &a, &b);
        for (_, r) in &rects {
            painter.rect_stroke(r.expand(1.0), 2.0, Stroke::new(1.5, red), egui::StrokeKind::Outside);
        }
        if let Some((_, first)) = rects.first() {
            crate::voice_bubble(painter, pos2(first.left() - 7.0, first.top() - 9.0), n, red);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use wordcraft_doc::{Document, Path};

    fn pos(p: u32, off: usize) -> Pos {
        Pos::new(StoryRef::Body, Path(vec![p]), off)
    }

    fn run(frames: &[(f32, bool, bool, (u32, usize))]) -> Vec<(f32, usize)> {
        let mut p = Pointing::new((pos(0, 0), pos(0, 0)));
        for &(t, pressed, down, (para, off)) in frames {
            let a = pos(para, off);
            p.observe(t, pressed, down, (a.clone(), a.clone()), || Some((a.clone(), a)));
        }
        p.pointers.into_iter().map(|x| (x.at, x.anchor.0.off)).collect()
    }

    #[test]
    fn a_quick_click_points_at_where_it_landed_not_where_the_caret_was() {
        // Press and release in one frame: the canvas moves the caret only after this frame's
        // extension hook ran, so the selection still says the old place here.
        let got = run(&[(1.0, true, false, (0, 0)), (1.06, false, false, (0, 12)), (1.12, false, false, (0, 12))]);
        assert_eq!(got, vec![(1.0, 12)]);
        // A double click on the same word: one pointer; again later: another.
        let got = run(&[(1.0, true, false, (0, 0)), (1.05, false, false, (0, 12)), (1.3, true, false, (0, 12)), (1.35, false, false, (0, 12))]);
        assert_eq!(got.len(), 1);
        let got = run(&[(1.0, true, false, (0, 0)), (1.05, false, false, (0, 12)), (3.0, true, false, (0, 12)), (3.05, false, false, (0, 12))]);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_click_points_at_the_word_and_a_selection_at_itself() {
        let mut s = Session::new(Document::from_text("The fee is GBP 9,750 a year.\nSecond  line."));
        s.sel = Selection::caret(pos(0, 16));
        let (a, b) = pointed_at(&s).unwrap();
        assert_eq!(anchor::para_text(&s.doc, &a.path).unwrap().get(a.off..b.off), Some("9,750"));
        s.sel = Selection { anchor: pos(0, 4), focus: pos(0, 7) };
        assert_eq!(pointed_at(&s).map(|(a, b)| (a.off, b.off)), Some((4, 7)));
        // Between words: the paragraph.
        s.sel = Selection::caret(pos(1, 7));
        let (a, b) = pointed_at(&s).unwrap();
        assert_eq!((a.off, b.off), (0, "Second  line.".len()));
    }

    #[test]
    fn numbering_matches_the_splice() {
        let x = (pos(0, 1), pos(0, 4));
        let y = (pos(1, 0), pos(1, 3));
        let ps = vec![Pointer { at: 2.0, anchor: y.clone() }, Pointer { at: 0.5, anchor: x.clone() }, Pointer { at: 3.0, anchor: x.clone() }];
        let t =
            craft_voice::Transcript::from_segments(vec![craft_voice::Segment { start: 0.0, end: 4.0, text: "this and that and this".into() }], "en");
        assert_eq!(numbered(&ps), craft_voice::splice(&t, &ps).refs);
    }
}
