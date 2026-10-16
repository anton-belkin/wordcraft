//! Is anyone listening? The connection status the Agent tab shows, from three kinds of signal:
//!
//! - **A check** ("is anyone there?"): `agent.ping` puts a system question in the queue, with
//!   `"kind": "ping"`. It goes out by every path a question does (list_questions,
//!   wait_for_questions, /events, the channel, the background agent) but never shows as a question,
//!   gets no badge and is never saved. An answer to it says an agent is really answering.
//! - **Passive listeners**: an `/events` stream or a channel stream (`GET /mcp`) connected now, a
//!   `wait_for_questions` call open now or in the last minute (HTTP or stdio), a background agent
//!   running.
//! - **The agent's last tool call.**
//!
//! Also the connection mode: how the app brings the agent in. It is the app's, never the file's.

use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

/// The check's text, as the agent reads it.
pub const PING_TEXT: &str = "Is anyone there? (connection check; answer \"here\")";
/// A check not answered within this long has no answer.
pub const PING_TIMEOUT_MS: u64 = 60_000;
/// A `wait_for_questions` call this recent counts as a listener.
pub const WAIT_WINDOW_MS: u64 = 60_000;

/// The connection check of a session (not in the layer: never saved).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ping {
    pub id: String,
    pub asked_at: u64,
    pub claimed: bool,
    pub answered_at: Option<u64>,
}

impl Ping {
    pub fn status(&self) -> &'static str {
        match (self.answered_at, self.claimed) {
            (Some(_), _) => "answered",
            (None, true) => "working",
            (None, false) => "pending",
        }
    }
}

/// How the app brings the agent in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The user connects an agent themselves (MCP over HTTP or stdio: Claude Code, Cowork, any client).
    #[default]
    Connect,
    /// New questions go to the user's own Claude Code session (Claude Code Channels).
    Channel,
    /// The app runs a background agent per document.
    Background,
    /// No agent: the server refuses the agent's calls.
    Off,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Connect, Mode::Channel, Mode::Background, Mode::Off];

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "connect" | "mcp" | "manual" => Some(Mode::Connect),
            "channel" | "session" | "my-session" => Some(Mode::Channel),
            "background" | "bg" => Some(Mode::Background),
            "off" | "none" => Some(Mode::Off),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Mode::Connect => "connect",
            Mode::Channel => "channel",
            Mode::Background => "background",
            Mode::Off => "off",
        }
    }
    /// The list item.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Connect => "I connect my agent",
            Mode::Channel => "Send to my Claude Code session",
            Mode::Background => "Background agent",
            Mode::Off => "Off",
        }
    }
    /// One line of explanation under it.
    pub fn explain(self) -> &'static str {
        match self {
            Mode::Connect => "MCP: Claude Code, Cowork or any client watches the questions",
            Mode::Channel => "Questions arrive in your open Claude Code session (Channels)",
            Mode::Background => "WordCraft runs Claude itself, one session per document",
            Mode::Off => "No agent: questions wait, the agent's calls are refused",
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(0);

pub fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Channel,
        2 => Mode::Background,
        3 => Mode::Off,
        _ => Mode::Connect,
    }
}

/// Sets the mode (the background agent's own module stops its agents when it is left).
pub fn store_mode(m: Mode) {
    MODE.store(
        match m {
            Mode::Connect => 0,
            Mode::Channel => 1,
            Mode::Background => 2,
            Mode::Off => 3,
        },
        Ordering::Relaxed,
    );
}

static EVENT_STREAMS: AtomicUsize = AtomicUsize::new(0);
static CHANNEL_STREAMS: AtomicUsize = AtomicUsize::new(0);
static WAITS_OPEN: AtomicUsize = AtomicUsize::new(0);
static LAST_WAIT: AtomicU64 = AtomicU64::new(0);

/// Which stream a listener holds open.
#[derive(Clone, Copy, Debug)]
pub enum Stream {
    Events,
    Channel,
    Wait,
}

fn counter(k: Stream) -> &'static AtomicUsize {
    match k {
        Stream::Events => &EVENT_STREAMS,
        Stream::Channel => &CHANNEL_STREAMS,
        Stream::Wait => &WAITS_OPEN,
    }
}

/// Counts a listener while it is alive.
pub struct Listening(Stream);

impl Listening {
    pub fn new(k: Stream) -> Listening {
        counter(k).fetch_add(1, Ordering::Relaxed);
        Listening(k)
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        // Every `Listening` added one.
        counter(self.0).fetch_sub(1, Ordering::Relaxed);
    }
}

/// A `wait_for_questions` poll reached the document (the app sees these from HTTP and stdio alike).
pub fn note_wait() {
    LAST_WAIT.store(crate::now_ms(), Ordering::Relaxed);
}

/// The passive signals now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listeners {
    pub events: usize,
    /// Channel streams (counted only in the channel mode, where they carry questions).
    pub channel: usize,
    /// A `wait_for_questions` is open or was in the last minute.
    pub waiting: bool,
    pub background: bool,
}

impl Listeners {
    pub fn any(&self) -> bool {
        self.events > 0 || self.channel > 0 || self.waiting || self.background
    }

    /// What is listening, in a few words.
    pub fn describe(&self) -> String {
        let mut v = Vec::new();
        if self.events > 0 {
            v.push(if self.events == 1 { "event stream".to_string() } else { format!("{} event streams", self.events) });
        }
        if self.channel > 0 {
            v.push("your Claude Code session".into());
        }
        if self.waiting {
            v.push("wait_for_questions".into());
        }
        if self.background {
            v.push("background agent".into());
        }
        v.join(", ")
    }
}

/// The passive signals; `background` is whether the app's own agent runs.
pub fn listeners(background: bool) -> Listeners {
    let now = crate::now_ms();
    let last = LAST_WAIT.load(Ordering::Relaxed);
    Listeners {
        events: EVENT_STREAMS.load(Ordering::Relaxed),
        channel: if mode() == Mode::Channel { CHANNEL_STREAMS.load(Ordering::Relaxed) } else { 0 },
        waiting: WAITS_OPEN.load(Ordering::Relaxed) > 0 || (last > 0 && now.saturating_sub(last) < WAIT_WINDOW_MS),
        background,
    }
}

/// The status line of the Agent tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// A check was answered (`at`: when it was asked).
    Answering {
        checked_at: u64,
        secs: u64,
    },
    /// A check is out, no answer yet.
    Checking,
    /// A check went unanswered.
    NoAnswer,
    /// A passive signal only.
    Listener,
    Nobody,
    Off,
}

/// A check answered this long ago still counts.
const ANSWER_FRESH_MS: u64 = 15 * 60_000;

pub fn status(ping: Option<&Ping>, l: &Listeners, now: u64) -> Status {
    if mode() == Mode::Off {
        return Status::Off;
    }
    match ping {
        Some(p) if p.answered_at.is_some() && now.saturating_sub(p.asked_at) < ANSWER_FRESH_MS => {
            let secs = p.answered_at.unwrap_or(p.asked_at).saturating_sub(p.asked_at) / 1000;
            return Status::Answering { checked_at: p.asked_at, secs };
        }
        Some(p) if p.answered_at.is_none() && now.saturating_sub(p.asked_at) < PING_TIMEOUT_MS => return Status::Checking,
        Some(p) if p.answered_at.is_none() => return Status::NoAnswer,
        _ => {}
    }
    if l.any() { Status::Listener } else { Status::Nobody }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_follows_the_check_then_the_listeners() {
        let l = Listeners::default();
        let now = 1_000_000;
        assert_eq!(status(None, &l, now), Status::Nobody);
        assert_eq!(status(None, &Listeners { events: 1, ..Default::default() }, now), Status::Listener);
        let mut p = Ping { id: "ping1".into(), asked_at: now - 5_000, ..Default::default() };
        assert_eq!(status(Some(&p), &l, now), Status::Checking);
        assert_eq!(status(Some(&p), &l, now + PING_TIMEOUT_MS), Status::NoAnswer);
        p.answered_at = Some(now - 2_000);
        assert_eq!(status(Some(&p), &l, now), Status::Answering { checked_at: now - 5_000, secs: 3 });
        assert_eq!(p.status(), "answered");
        for m in Mode::ALL {
            assert_eq!(Mode::parse(m.id()), Some(m));
        }
    }

    #[test]
    fn listeners_count_while_alive() {
        let before = listeners(false).events;
        {
            let _a = Listening::new(Stream::Events);
            assert_eq!(listeners(false).events, before + 1);
        }
        assert_eq!(listeners(false).events, before);
    }
}
