# WordCraft fork: the agent layer

This fork adds a layer of interaction with the user's own agent (Claude Code, Claude Desktop, any
MCP client) inside the document, so the person who has to sign off on a document can review it
faster: questions on passages answered in the background, the agent's explanations and sources in
the margin, a privacy gate for client data, templates rendered per client without the agent ever
seeing the client, and a history of who changed what. It is the WordCraft counterpart of the
GridCraft agent layer (`anton-belkin/gridcraft`, branch `agent-layer`); the MCP tool names are the
same, so one agent skill pattern works for both.

The fork is built to be **rebased onto upstream often and cleanly**. Read "Keeping the fork alive"
before changing anything outside the agent crates.

## What is where

| Path | What | Upstream? |
| --- | --- | --- |
| `crates/agent/` (`wordcraft-agent`, L5) | The layer: data model and storage in the `.docx`, quote anchors, the privacy mask, templates and the CRM fixture, `agent.*` commands, the observer (save, history, template preview), the in-app MCP server, and `wordcraft-agent-mcp` (the same over stdio) | fork only |
| `crates/agent-ui/` (`wordcraft-agent-ui`, L6) | The Agent tab on the ribbon (groups Agent, Client data, Context), the Agent pane (the passage's questions, answers, notes, history), badges and margin notes on the page, context-menu items; `examples/agent_demo.rs` renders the window offscreen with the real MCP server | fork only |
| `apps/wordcraft/src/agent.rs` | Starts the layer in the desktop app (`--agent-port`, `--no-agent`, `--agent-demo`) | fork only |
| `agent-layer/` | The agent's skill, the end-to-end demo script, the rebase script | fork only |
| `crates/engine/src/ext.rs` | **Hook**: extension commands, command observers, per-session extension state | new file in an upstream crate |
| `crates/ui-egui/src/ext.rs` | **Hook**: UI extensions (side panel, page overlay, context menu, ribbon tabs, per-frame) | new file in an upstream crate |
| lines marked `fork:agent` | The only edits to upstream files | patch |

### The patch to upstream files

Every edit to a file that exists upstream carries a `fork:agent` comment. `git grep -n fork:agent`
lists them; there are about twenty lines, all insertions but one:

- `crates/engine/src/lib.rs`: `pub mod ext;`.
- `crates/engine/src/cmd/mod.rs`: `registry()` appends the extension commands.
- `crates/engine/src/session.rs`: the `ext` field of `Session` (declared, initialised, listed in
  `edit_snapshot`'s destructuring), and the observers called before and after each command in
  `Session::run`.
- `crates/ui-egui/src/lib.rs`: `pub mod ext;`, `ext::frame` and `ext::side_panels` in `WordApp::ui`.
- `crates/ui-egui/src/ribbon.rs`: extension tabs appended to the tab strip, and drawn by their
  extension (`ext::ribbon_tab`).
- `crates/ui-egui/src/keytips.rs`: extension tabs' keytips (`tab_letter`, `badge_tabs`); the Agent
  tab is Alt, A (F10, A on macOS).
- `crates/ui-egui/src/canvas.rs`: `ext::paint_pages` after Find's highlights; the context menu's
  closure also calls `ext::context_menu` (the one changed line).
- `apps/wordcraft/src/main.rs`, `apps/wordcraft/Cargo.toml`: wiring (seven lines, two deps; the
  first line of `main` hands `--agent-stdio` to the agent layer).
- `Cargo.toml`: the two workspace dependencies.
- `xtask/src/layers.rs`: the two crates in the layer table and their order within L5/L6.

The hooks know nothing about agents: they are generic extension points, written to be proposed
upstream. Each one accepted upstream removes its lines from the patch.

## Design decisions (and why)

- **Commands, not side channels.** Everything the layer does is an `agent.*` command registered
  through `wordcraft_engine::ext`, so the UI, the control channel, the CLI and MCP reach it like
  any other command, and `Session::run` keeps catching panics. The layer's commands are `pure()`
  (no undo step: undoing an edit must never take away a question or an answer); they mark the
  document dirty themselves.
- **The layer lives in the session's extension state** (`Session::ext`), not in the document, so
  undo and redo leave it alone. It is written into the document as a custom XML part
  (`urn:wordcraft:agent-layer:v1`, JSON in CDATA, `customXml/wordcraftAgent.xml`) just before
  `file.save` / `file.saveAs`, and read back when a document is opened. WordCraft keeps custom XML
  parts on open and save (`docx` passthrough), and so does Word, so the layer travels with the file.
- **Anchors are quotes, not marks.** A question or note keeps its paragraph, offsets and the quoted
  text; after every changing command (and undo/redo) it is found again: at its place, elsewhere in
  the paragraph, in nearby paragraphs, then by its first and last words. Nothing is written into the
  text, so anchors survive any edit and a round trip through Word, and also resolve in a template's
  rendering.
- **Privacy is enforced where the agent reads.** Client-data ranges read as `{{label}}`
  placeholders (the same text keeps the same placeholder); the agent addresses text as it read it
  (paragraph number + quote) and the layer maps that back, refusing ranges that touch client data.
  Every string going to the agent (questions the reader typed, history, quotes) is scrubbed of the
  real text of client data and of the CRM's identifying values. `agent.exec` refuses commands that
  read or search the real text (`select.text`, find/replace, copy/paste, `document.*`, `file.*`),
  checks the selection, and takes back any change that alters client data. Only a person lifts a
  mark. Not covered: the control port (7981) and the `.docx` on disk still hold the real text.
- **Templates are previews.** `agent.render` shows the template rendered with a CRM client
  (`{{client.name}}`, `{% for ubo in client.ubos %}…{% endfor %}`, `{% if client.vip %}…`), read only;
  the template is kept underneath. The agent reads and edits the template (its `run_command` is
  applied to the template and the preview re-renders at once); Save and Undo act on the template.
  The reader asks on the client's copy without switching: the renderer keeps a source map (each
  rendered block's template block, loops and iteration, and where values were filled in), and the
  question's place goes on the template text it came from. That is exact for a passage of template
  paragraphs shown once, and a selection inside a filled-in value covers its placeholder. Inside a
  loop's repetition the place covers the loop's body, the box says "Asked on the template's loop
  block", and the copy shows the marker on every repetition. The agent still gets only the
  template and placeholders.
  The CRM is three invented clients built in code (`crates/agent/src/crm.rs`).
- **The MCP server runs inside the app** (`http://127.0.0.1:7982/mcp`, Streamable HTTP, JSON
  responses; GridCraft uses 7980) and reaches the document through the app's own loopback control
  channel. `/events` streams new questions, one JSON line each, for an agent's background monitor
  (`curl -sN http://127.0.0.1:7982/events`).
- **Is anyone listening?** (`presence.rs`, internal: the UI never shows the check or its wording)
  The Ready lamp of the Context group comes from a quiet check and from passive signals. The check
  (`agent.ping`) puts a system question in the queue, `"kind": "ping"`: it goes out by every path
  (list_questions, wait_for_questions, `/events`, the channel, the background agent), is never
  shown as a question, has no badge and is never saved; an answer within 60 s means a listener is
  really answering. Passive: a conversation attached to this document (AItoCraft `attach`), an
  `/events` or channel stream connected, a `wait_for_questions` poll in the last minute (its calls
  carry `waiting`, so HTTP and stdio count alike), the background agent running. Stream polls carry
  `poll`, so neither counts as the agent's last tool call.
- **Voice questions** (`crates/agent-ui/src/voice.rs`, native only; the same as GridCraft's):
  hold ⌥ (Alt) alone, or click **Ask by Voice** on the Agent tab, speak, and click or select the
  passages you mean while speaking; release (or click again) and the question goes on at once. A
  click points at the word under the caret (the paragraph between words), a selection at itself;
  each passage pointed at becomes a place of the question's topic (TOPICS.md), and `⟦n⟧` marks in
  the text where it was pointed at, placed by time (`craft_voice::splice`). The question is on the
  document at once ("transcribing", not with the agent) and goes to the agent without preview or
  confirmation when its text is there (`agent.spoken`); the agent reads `⟦1: §12 “quote”⟧` and
  `refs`. A failed transcription is a flag with Retry and Dismiss (`agent.removeQuestion`). Local
  Whisper through [craft-voice](https://github.com/anton-belkin/craft-voice) (a git dependency at
  a fixed rev), model Off / Small / Turbo on the ribbon (an app setting, `voice.json`, downloaded
  on first use). Audio stays in memory until its text is there and is never written. Releasing ⌥
  after a question doesn't open keytips. `agent-layer/rebase.sh` puts back the yanked `zip 8.6.0`
  that candle needs in an upstream lock.
- **The Context group** (Agent tab; the same in GridCraft and DeckCraft, `context.rs`): what the
  agent knows. A large button shows the mode with the lamp's dot; its menu has **No context**
  ("Only this document. Runs in the background." [Document]), **Claude Cowork** ("Your Cowork chat,
  its project and memory." [Document][Your chat]) and **Claude Code** ("Your Claude Code session or
  a project folder." [Document][Project folder]). Beside it: No context has a **Web** checkbox (the
  background client may also use WebSearch and WebFetch); Claude Code has a **My session | Project
  folder** toggle; each has the **Ready lamp** (green Ready: a listener for this document, or the
  background client usable; amber Waiting: after a Get ready step until someone comes; grey Not
  ready) and **Get ready ▾**, whose main click runs the next step not done and whose list ticks the
  done ones: No context › Log in to Claude (opens a terminal with `claude auth login`; shown only
  while the CLI isn't logged in, from `claude auth status`); Cowork › Install AItoCraft (`aitocraft
  --install-desktop`), Ask Cowork to collaborate (copies the phrase); My session › Install AItoCraft
  (`claude mcp add --scope user aitocraft -- <hub>`, done when `claude mcp get aitocraft`
  succeeds), Ask my session to collaborate (copies `claude
  --dangerously-load-development-channels server:aitocraft`); Project folder › Choose folder… (the
  system's chooser; the path is shown; the background client runs there and sees that folder's
  CLAUDE.md, memory and files). The lamp checks by itself and sends the quiet check when something
  is connected but hasn't said it is here for this document. The context is the document's, saved
  in its agent layer (`context`), and a person's change is an undo step (`agent.setContext`); a new
  document starts with the last context used in the app (`context.json` in the settings folder). A
  project folder written in a file is used only once chosen on this machine (`folders` in the
  settings folder): a document someone sent can't point the agent at a folder. Right-click the
  mode button for the background session (Open in Claude Desktop, New session, Stop) and Details,
  which keeps the per-app ways in without AItoCraft (`wordcraft --agent-stdio`, HTTP, the monitor
  command, the Claude Desktop entry).
- **Topics** (`~/aitocraft/TOPICS.md`, layer format v2; v1 files migrate on load). A topic is one
  question or theme: places (bookmarks `_GcPlace_<id>` in the document, with a quote anchor to fall
  back on), a status (Open, Revise, Agreed, For information) with text, who and when, notes, and the
  discussion. Numbered like footnotes by the first place (3, places 3.1, 3.2). Only a person sets
  Agreed; the agent proposes it. Copy and paste duplicates a place into the same topic (the
  selection over a whole place takes its bookmark along, as in Word), cut and paste moves it,
  deleting its text leaves a collapsed bookmark and the text ("removed here: “…”"). Links inside
  texts (`[§12](para:12)`, `quote:`, `place:`, `topic:`, `note:`, plain `§12`) are stored as anchors
  and shown with current numbers (`links.rs`). The page shows only tiny numbered circles in the
  margin, coloured by status (blue and pulsing while the agent works, a dot for an unread answer);
  the pane has Selection (the question box at the bottom), Overview (the sign-off line) and History.
  "Save copy to send" (Agent tab) drops the discussion unless "include discussion" is ticked.
  **Undo** (`undo.rs`): WordCraft's undo restores whole snapshots, so every person action on the
  layer (ask, status, pin, remove a note, reject a place, add a place, client data) is a changing
  command whose change is its bookmarks plus a step counter in an unsaved passthrough key; ⌘Z
  brings the counter back and the layer applies the step's inverse (an undone question disappears,
  or is withdrawn if the agent had it; redo sends it again; an answered one comes back answered).
  The agent's contributions are never steps: when an undo restores a snapshot without the agent's
  bookmarks, the layer puts them back from their last known position. No engine change.
- **AItoCraft** (`~/aitocraft/PROTOCOL.md`): one MCP hub, configured once in Claude Desktop /
  Cowork and Claude Code, serves every open document of every Craft app; the user picks the
  context by starting the conversation where the work belongs ("we're working on contract.docx").
  WordCraft's side:
  - *Registry* (`registry.rs`): `ArtCraft/agents/wordcraft-<pid>.json` (`protocol`, `app`,
    `appTitle`, `version`, `pid`, `mcp`, `heartbeat`, `documents`), written atomically at start, on
    change (at most once a second) and every 10 s, deleted on exit (`on_exit` and the macOS
    terminate path). Titles and paths only. A WordCraft window holds one document, so a registry
    file lists one; its id is `wordcraft:<pid>:<document uid>` (a new id when another file is
    opened in the window). `focusedAt` (when the window last had the focus) is added for the hub.
  - *MCP*: every tool takes an optional `document` (an id of another document is an error:
    `document <id> is not open in WordCraft`); `list_documents`, `attach {document, label}` →
    `{listener, document, title, app}`, `detach {listener}`; `listener` on list_questions,
    wait_for_questions, claim_question and answer_question (refreshes it; dropped after 90 s; an
    unknown one is told to attach again); every question carries `document` and `documentTitle`;
    `GET /events?document=<id>`; a ping answer records the listener that answered.
- **The connection modes under the contexts** (`presence::Mode`; `--agent-mode
  connect|channel|background|off` sets the app's last context, `agent.agentMode` the document's):
  Claude Cowork is *connect*, Claude Code › My session is *channel*, No context and Claude Code ›
  Project folder are *background*. The background client starts only with a question the reader
  asks.
  - *Connect*: MCP over HTTP or stdio (Claude Code, Cowork, any client), through AItoCraft.
  - *Send to my Claude Code session* (Claude Code Channels, research preview): the server declares
    `experimental["claude/channel"]` and streams each new question on `GET /mcp` (SSE) as
    `notifications/claude/channel`; `wordcraft --agent-stdio` bridges them to stdout (Claude Code
    spawns channel servers over stdio). Without AItoCraft (Details): `claude mcp add
    wordcraft-channel -- <wordcraft> --agent-stdio`, then `claude
    --dangerously-load-development-channels server:wordcraft-channel`. Interactive sessions only
    (`-p` ignores channels).
  - *Background agent* (`runner.rs`, demoted: it decides the context itself): one `claude -p
    --input-format stream-json --output-format stream-json --verbose` per document, by default in a
    neutral, empty folder (`ArtCraft/agent-sessions/`, so no project's CLAUDE.md, memory or settings
    become its context), or the project folder of Claude Code › Project folder; with No context ›
    Web it may also use `WebSearch` and `WebFetch`. A session is resumed only in the folder it
    ran in (`agentCwd`); a change of folder or web restarts the client. Named `WordCraft · <file>` (`--name`), with
    `--mcp-config` + `--strict-mcp-config` (only the WordCraft server), `--allowedTools
    "mcp__wordcraft__*"`, the skill as `--append-system-prompt-file`, `--max-turns 40`,
    `--max-budget-usd 3.00`. It starts with the first waiting question and gets each new one once;
    its session id is kept in the layer (`agentSession`), so reopening the document resumes it
    (`--resume`); a failed resume or New session starts fresh from a summary of the history. Stop
    is end of input, SIGINT, then kill. Never `--bare`. `WORDCRAFT_AGENT_COMMAND` picks another
    CLI.
  - *Off* (`agent.agentMode`, not in the group): the agent's calls are refused ("the agent
    connection is off").

## Running it

```bash
cargo run --release -p wordcraft -- --agent-demo
```

The agent server starts with the app (and the control channel on 7981 with it); `--agent-demo`
opens the engagement-letter template. Connect Claude Code:

```bash
claude mcp add --transport http wordcraft http://127.0.0.1:7982/mcp
```

and give it the skill in `agent-layer/skill/`.

**Claude Desktop / Cowork (stdio).** Clients that start local MCP servers as processes run the app
itself in stdio mode: `wordcraft --agent-stdio [--control 127.0.0.1:7981]` serves the same tools
over stdio and exits when stdin closes, reaching the running app through its control channel (it
connects on first use, so Claude can start before WordCraft). The config entry, server name
`wordcraft`:

```json
"wordcraft": {"command": "<path to the wordcraft executable>", "args": ["--agent-stdio", "--control", "127.0.0.1:7981"]}
```

The **Claude Desktop: add WordCraft** item (Agent tab ▸ Context ▸ right-click ▸ Details) writes it into
`~/Library/Application Support/Claude/claude_desktop_config.json` (Windows: `%APPDATA%\Claude\…`),
keeping every other server and setting and copying the old file to `….bak-wordcraft-<time>`. Claude
Desktop keeps that file in memory and writes it back when it quits, so an entry added while it runs
is lost: if Claude is running, the button waits until it has quit (Cmd+Q), then writes; open Claude
again and the tools are there (`crates/agent/src/cowork.rs`). There is no event stream over stdio:
the agent listens with `wait_for_questions` in a loop (each call waits up to 50 s).
`wordcraft-agent-mcp` (`crates/agent/src/bin`) is the same server as a separate small binary. Offscreen, with the real server and `curl` as the
agent (works with a locked screen):

```bash
cargo run --release -p wordcraft-agent-ui --example agent_demo -- agent-layer/demo/end_to_end.jsonl /tmp/shots
```

(the PNGs and a saved `.docx` go to `/tmp/shots`; the script's `SHOTS` stands for that folder).

## Keeping the fork alive

```bash
agent-layer/rebase.sh          # rebase onto upstream/main, check markers, run the gates
agent-layer/rebase.sh --full   # the same with `cargo xtask ci` as the gate
```

The script fetches `upstream`, rebases the current branch onto `upstream/main` (a `Cargo.lock`
conflict is resolved by taking upstream's lock and letting cargo add our crates), checks that every
`fork:agent` marker is still there, then runs fmt, clippy, the agent, engine, MCP and UI tests,
assets, layers, wasm and a release build. When it stops on a conflict, the conflict can only be in
one of the marked lines above: keep upstream's version and put the marked line back.

Rules that keep it this way:

1. New behaviour goes in `crates/agent*` or behind a hook, never into an upstream file.
2. Need a new hook? Make it generic (no agent vocabulary), put it in the crate's `ext.rs`, call it
   with one marked line, list it above.
3. Use public engine and UI APIs. When upstream renames one, the build breaks in `crates/agent*`
   with a clear error instead of a conflict. The one private detail used is the `docx` crate's
   passthrough bookkeeping keys for custom XML parts; `the_layer_survives_save_and_reopen` pins it.
4. Commits: hooks and wiring in their own commits, separate from the agent crates.

## Status

Prototype. Done for WordCraft: questions on passages with live badges (waiting / the agent is on
it / answer ready), the in-app MCP server and event stream, agent notes in the margin with sources
(URL + summary) and an Ask button, prepared answers (`faq` notes), the privacy gate with
placeholders, templates with loops and conditions rendered per client from a local CRM fixture,
decision history and audit trail (person or agent, reason, source), storage in the `.docx`.

Not yet: headers, footers and footnotes are not in the agent's view (body and tables only);
questions asked on a client's rendering are anchored to the rendered text; the safe-file mode (a
`.docx` without the client data); export to a self-contained web page; DeckCraft.
