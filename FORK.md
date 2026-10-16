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
| `crates/agent-ui/` (`wordcraft-agent-ui`, L6) | The Agent tab on the ribbon (groups Agent, Client data, Connection), the Agent pane (the passage's questions, answers, notes, history), badges and margin notes on the page, context-menu items; `examples/agent_demo.rs` renders the window offscreen with the real MCP server | fork only |
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
  The CRM is three invented clients built in code (`crates/agent/src/crm.rs`).
- **The MCP server runs inside the app** (`http://127.0.0.1:7982/mcp`, Streamable HTTP, JSON
  responses; GridCraft uses 7980) and reaches the document through the app's own loopback control
  channel. `/events` streams new questions, one JSON line each, for an agent's background monitor
  (`curl -sN http://127.0.0.1:7982/events`).

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

The **Connect Cowork** button (Agent tab ▸ Connection) writes it into
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
