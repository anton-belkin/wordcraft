//! The agent's MCP surface: JSON-RPC 2.0 (MCP `2025-06-18`) whose tools map onto `agent.*`
//! commands of the running app. The tool names are the ones GridCraft's agent layer uses, so one
//! agent skill works across the Craft apps. Every read is masked; no tool returns client data.

use serde_json::{Value, json};
use wordcraft_mcp::Backend;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "You are connected to the WordCraft document the user has open, through its agent layer. \
The user reviews and signs off on this document; your job is to make that fast. They ask questions on passages: watch for them \
(GET http://127.0.0.1:<port>/events streams one line per new question, or call wait_for_questions), claim_question, \
investigate with get_context / read_document, then answer_question with links to the paragraphs and sources you used \
(href \"para:12\", \"note:n3\" or https URLs with a one-line summary). Leave prepared explanations and answers to likely questions \
with add_note, and log decisions (log_decision). Change the document only with run_command, pointing at text with `at` \
{paragraph, quote} and giving a reason and a source. PRIVACY: text marked as client data reads as {{placeholder}} in everything \
you get (get_context.clientData lists them); refer to it by its placeholder and never try to find the real value. You cannot write \
into client data. When the document is a template (get_context.template), edit it with placeholders from that vocabulary: the user \
renders it for a client themselves; you never see a client's values. A question with kind ping is a connection check: answer it at \
once with \"here\", don't investigate. TOPICS: every question belongs to a topic (numbered like footnotes, places 3.1, 3.2) with \
a status (open, revise, agreed, info), notes and a discussion. Link inside the text ([§12](para:12), [3.2](place:3.2), [topic 3](topic:3)); \
add places that a change here affects (add_places, with a reason); leave notes that will still matter after the chat (add_note with \
topic); never mark Agreed: propose it (propose_agreed). A question with status withdrawn was taken back by the user: drop it. \
PROPOSALS: in a topic, change the document only by proposing: run_command with `topic` (and `option` A/B/C when there is a real \
choice; describe_option gives each a title and a why). The document doesn't change; the person applies one option in the document \
(never ask for confirmation in the chat). Reads mark what pending proposals touch (pending); questions carry pendingNearby: don't \
contradict those. A rejected proposal's comment arrives as a follow-up question; an outdated one must be redone. \
VOICE: ⟦n⟧ in a question is what the user pointed at while speaking (expanded as ⟦n: §12 “quote”⟧, each a place of the topic), \
see refs; resolve \"this\", \"that one\", \"это\" from those and the context.";

/// Tools that take a `listener` (AItoCraft: a call carrying it keeps the conversation attached).
const LISTENER_TOOLS: &[&str] = &["list_questions", "wait_for_questions", "claim_question", "answer_question"];

fn tool(name: &str, desc: &str, mut props: Value, required: &[&str], read_only: bool) -> Value {
    // Every tool may name the document (an AItoCraft registry id); without it, the open one.
    if let Value::Object(o) = &mut props {
        o.entry("document").or_insert_with(|| s("Document id from list_documents / attach (e.g. \"wordcraft:4121:1\"); omit for the open document"));
        if LISTENER_TOOLS.contains(&name) {
            o.entry("listener").or_insert_with(|| s("Your listener id from attach (keeps you attached; questions of that document only)"));
        }
    }
    json!({
        "name": name,
        "description": desc,
        "inputSchema": {"type": "object", "properties": props, "required": required, "additionalProperties": false},
        "annotations": {"readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false},
    })
}

fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}
fn n(desc: &str) -> Value {
    json!({"type": "number", "description": desc})
}
fn links() -> Value {
    json!({"type": "array", "description": "Links: {label, href, summary?}. href is https://…, para:<n> or note:<id>; give sources a one-line summary", "items": {
        "type": "object", "properties": {"label": {"type": "string"}, "href": {"type": "string"}, "summary": {"type": "string"}}, "required": ["label", "href"]}})
}
fn para() -> Value {
    n("Paragraph number, as read_document shows it")
}
fn quote() -> Value {
    s("Exact text inside that paragraph, as read_document shows it (placeholders included); omit for the whole paragraph")
}

pub fn tools() -> Vec<Value> {
    vec![
        tool(
            "get_context",
            "Overview of the open document: title, outline (headings with paragraph numbers), client-data placeholders, the template vocabulary if it is a template, open questions, recent history.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "read_document",
            "Read paragraphs (numbered, with style). Client data reads as {{placeholder}}. A paragraph that a proposal not yet applied would change carries pending: [{topic, option, summary}]. view \"proposal\" with topic (and option) reads that proposal's draft instead.",
            json!({"from": n("First paragraph (default 0)"), "count": n("How many (default 400)"), "view": {"type": "string", "enum": ["document", "proposal"]}, "topic": s("With view proposal: the topic"), "option": s("With view proposal: the option (default the first)")}),
            &[],
            true,
        ),
        tool(
            "list_questions",
            "Questions the user asked on passages. status: pending (new), working (claimed), answered, open (pending+working) or all. A spoken question carries ⟦n: §12 “quote”⟧ where the user pointed at a passage while speaking; refs lists them.",
            json!({"status": {"type": "string", "enum": ["pending", "working", "answered", "open", "withdrawn", "all"]}}),
            &[],
            true,
        ),
        tool(
            "wait_for_questions",
            "Block until there is a pending question (or the timeout passes) and return the pending ones. An alternative to watching /events.",
            json!({"timeout_seconds": n("Up to 50 (default 25)")}),
            &[],
            true,
        ),
        tool(
            "claim_question",
            "Take a pending question so the user sees it is being worked on.",
            json!({"id": s("Question id, e.g. \"q3\"")}),
            &["id"],
            false,
        ),
        tool(
            "answer_question",
            "Answer a question; the passage shows \"answer ready\". Answer first, short paragraphs; link the paragraphs and sources you used.",
            json!({"id": s("Question id"), "text": s("The answer (plain text, short paragraphs)"), "links": links()}),
            &["id", "text"],
            false,
        ),
        tool(
            "add_note",
            "Leave a note on a topic, one that will still matter after the chat: an explanation, a source (url + summary), an assumption or a decision. Without `topic`, on a passage (`paragraph`, `quote`): a topic of its own, For information. Write links inside the text: [§12](para:12), [the fee clause](quote:12/twelve months), [3.2](place:3.2), [topic 3](topic:3), [NOAA](https://…).",
            json!({"topic": s("Topic id or number"), "paragraph": para(), "quote": quote(), "title": s("Short title"), "text": s("Note text, links inside"), "kind": {"type": "string", "enum": ["explanation", "source", "assumption", "decision", "faq"]}, "url": s("For a source: its URL"), "summary": s("For a source: one line on what it says")}),
            &["text"],
            false,
        ),
        tool(
            "list_topics",
            "The document's topics in order (numbers like footnotes: 3, places 3.1, 3.2): places (paragraph + quote, removed ones flagged), status and its text, note titles, unread answers, the sign-off line.",
            json!({"status": {"type": "string", "enum": ["open", "revise", "agreed", "info"]}}),
            &[],
            true,
        ),
        tool(
            "get_topic",
            "Everything about a topic: places, status, notes with their text, the discussion so far, the agent's proposal.",
            json!({"topic": s("Topic id or number")}),
            &["topic"],
            true,
        ),
        tool(
            "add_places",
            "Attach places to a topic when a change here implies changes there, each with a short reason (\"depends on 3.1 via the fee clause\"); the person sees them on the document.",
            json!({"topic": s("Topic id or number"), "places": {"type": "array", "items": {"type": "object", "properties": {"paragraph": para(), "quote": quote(), "through": n("Last paragraph, for a place over several"), "reason": s("Why it belongs here")}, "required": ["paragraph", "reason"]}}}),
            &["topic", "places"],
            false,
        ),
        tool(
            "widen_place",
            "Widen a place to what the answer is really about (the whole clause, several paragraphs), and say why.",
            json!({"place": s("Place label, e.g. \"3.1\""), "to": {"type": "object", "properties": {"paragraph": para(), "quote": quote(), "through": n("Last paragraph")}, "required": ["paragraph"]}, "reason": s("Why")}),
            &["place", "to", "reason"],
            false,
        ),
        tool(
            "set_status",
            "Set a topic's status: open (a question, no decision), revise (something here must change), info (nothing to decide), with a short text (links inside). Agreed is the person's: use propose_agreed.",
            json!({"topic": s("Topic id or number"), "status": {"type": "string", "enum": ["open", "revise", "info"]}, "text": s("The status in a sentence")}),
            &["topic", "status"],
            false,
        ),
        tool(
            "propose_agreed",
            "Suggest that a topic is settled; the person confirms it with one click.",
            json!({"topic": s("Topic id or number"), "reasons": s("Why it can be agreed (links inside)")}),
            &["topic", "reasons"],
            false,
        ),
        tool(
            "log_decision",
            "Record a decision in the document's history (why the document is the way it is), optionally on a passage.",
            json!({"text": s("The decision and its reason"), "paragraph": para(), "quote": quote(), "source": s("What it is based on")}),
            &["text"],
            false,
        ),
        tool("get_layer", "The whole agent layer: questions, notes, client-data placeholders, history.", json!({}), &[], true),
        tool(
            "get_file_context",
            "The file's own context: what the document is, for whom, house rules, background, facts gathered. get_context and every question carry a short summary of it.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "add_file_context",
            "Add to the file's own context (kept in the file, kept when it is sent): something about the document as a whole, not a place. Returns its id.",
            json!({"title": s("A few words"), "kind": s("about, audience, rules, background, fact…"), "text": s("The context (links inside)")}),
            &["title", "text"],
            false,
        ),
        tool(
            "update_file_context",
            "Change an entry of the file's context.",
            json!({"id": s("Entry id"), "title": s("New title"), "kind": s("New kind"), "text": s("New text")}),
            &["id"],
            false,
        ),
        tool("remove_file_context", "Remove an entry of the file's context.", json!({"id": s("Entry id")}), &["id"], false),
        tool(
            "describe_option",
            "Name an option of a topic's proposal and say why: the person sees the title and the why beside its changes.",
            json!({"topic": s("Topic id or number"), "option": s("A, B, C…"), "title": s("A few words"), "why": s("Why this option (links inside)")}),
            &["topic", "option", "title"],
            false,
        ),
        tool(
            "discard_proposal",
            "Drop an option of a topic's proposal, or the whole proposal (to redo it).",
            json!({"topic": s("Topic id or number"), "option": s("A, B…; omit for the whole proposal")}),
            &["topic"],
            false,
        ),
        tool(
            "run_command",
            "Change the document with a WordCraft command (text.insert, format.bold, para.style…; see list_commands). `at` selects the text first. Logged as yours with the reason and source. Commands that read or search the real text are refused. With `topic` the document does NOT change: the command is recorded in that topic's proposal (option A unless `option` says otherwise) and checked on its draft; the person applies it in the document. In a topic, always propose this way; don't ask for confirmation in the chat.",
            json!({"command": s("Command id"), "params": {"type": "object"}, "at": {"type": "object", "description": "{paragraph, quote?, occurrence?}: the text to act on", "properties": {"paragraph": para(), "quote": quote(), "occurrence": n("Which match of the quote (0-based)")}}, "reason": s("Why"), "source": s("What it is based on"), "topic": s("Propose in this topic (id or number) instead of changing the document"), "option": s("The option it belongs to: A (default), B, C… when there is a real choice")}),
            &["command", "reason"],
            false,
        ),
        tool(
            "list_documents",
            "The documents open in WordCraft (id, title, path, open questions, who listens), as in the AItoCraft registry.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "attach",
            "Bind this conversation to a document: questions asked there reach you. Returns your listener id: pass it (and the document id) on list_questions, wait_for_questions, claim_question and answer_question.",
            json!({"label": s("What the user sees, e.g. \"Cowork · Acme\" or \"Claude Code · contracts\"")}),
            &[],
            false,
        ),
        tool("detach", "Stop listening for a document's questions.", json!({"listener": s("Your listener id from attach")}), &["listener"], false),
        tool(
            "list_commands",
            "List WordCraft commands that change the document (id, label, params).",
            json!({"search": s("Substring to filter on")}),
            &[],
            true,
        ),
    ]
}

/// Runs `command` for the agent, carrying the call's `document` and `listener` along.
fn exec_with(b: &mut dyn Backend, command: &str, mut params: Value, args: &Value) -> Result<Value, String> {
    if let Value::Object(o) = &mut params {
        for k in ["document", "listener"] {
            if let Some(v) = args.get(k).filter(|v| v.is_string()) {
                o.entry(k).or_insert_with(|| v.clone());
            }
        }
        o.insert("actor".into(), json!("agent"));
        o.insert("masked".into(), json!(true));
    }
    b.call("engine.execute", json!({"command": command, "params": params}))
}

fn pick(args: &Value, keys: &[&str]) -> Value {
    let mut o = serde_json::Map::new();
    for k in keys {
        if let Some(v) = args.get(*k) {
            o.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(o)
}

pub fn call(b: &mut dyn Backend, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "list_documents" => exec_with(b, "agent.documents", json!({}), args),
        "attach" => exec_with(b, "agent.attach", pick(args, &["label"]), args),
        "detach" => exec_with(b, "agent.detach", pick(args, &["listener"]), args),
        "get_context" => exec_with(b, "agent.context", json!({}), args),
        "read_document" | "read_range" => exec_with(b, "agent.read", pick(args, &["from", "count", "view", "topic", "option"]), args),
        "list_questions" => exec_with(b, "agent.questions", pick(args, &["status"]), args),
        "wait_for_questions" => {
            let secs = args.get("timeout_seconds").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(25.0).clamp(0.0, 50.0);
            let mut polls = (secs * 2.0) as u32;
            loop {
                let v = exec_with(b, "agent.questions", json!({"status": "pending", "waiting": true}), args)?;
                if v.as_array().is_some_and(|a| !a.is_empty()) || polls == 0 {
                    return Ok(v);
                }
                polls -= 1;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        "claim_question" => exec_with(b, "agent.claim", pick(args, &["id"]), args),
        "answer_question" => exec_with(b, "agent.answer", pick(args, &["id", "text", "links"]), args),
        "add_note" => exec_with(
            b,
            "agent.note",
            pick(args, &["topic", "paragraph", "quote", "occurrence", "text", "title", "kind", "links", "url", "summary"]),
            args,
        ),
        "list_topics" => exec_with(b, "agent.topics", pick(args, &["status"]), args),
        "get_topic" => exec_with(b, "agent.topic", pick(args, &["topic"]), args),
        "add_places" => exec_with(b, "agent.addPlaces", pick(args, &["topic", "places"]), args),
        "widen_place" => exec_with(b, "agent.widenPlace", pick(args, &["place", "to", "reason"]), args),
        "set_status" => exec_with(b, "agent.topicStatus", pick(args, &["topic", "status", "text"]), args),
        "propose_agreed" => exec_with(b, "agent.proposeAgreed", pick(args, &["topic", "reasons"]), args),
        "log_decision" => exec_with(b, "agent.decision", pick(args, &["text", "paragraph", "quote", "source"]), args),
        "get_layer" => exec_with(b, "agent.layer", json!({}), args),
        "run_command" => exec_with(b, "agent.exec", pick(args, &["command", "params", "at", "reason", "source", "topic", "option"]), args),
        "describe_option" => exec_with(b, "agent.describeOption", pick(args, &["topic", "option", "title", "why"]), args),
        "get_file_context" => exec_with(b, "agent.fileContext", json!({}), args),
        "add_file_context" => {
            let mut p = pick(args, &["title", "kind", "text"]);
            p["op"] = json!("add");
            exec_with(b, "agent.fileContextEdit", p, args)
        }
        "update_file_context" => {
            let mut p = pick(args, &["id", "title", "kind", "text"]);
            p["op"] = json!("update");
            exec_with(b, "agent.fileContextEdit", p, args)
        }
        "remove_file_context" => {
            let mut p = pick(args, &["id"]);
            p["op"] = json!("remove");
            exec_with(b, "agent.fileContextEdit", p, args)
        }
        "discard_proposal" => exec_with(b, "agent.discardProposal", pick(args, &["topic", "option"]), args),
        "list_commands" => {
            let all = b.call("engine.commands", json!({}))?;
            let q = args.get("search").and_then(Value::as_str).unwrap_or("").to_lowercase();
            let list: Vec<Value> = all
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|c| {
                            let id = c.get("id").and_then(Value::as_str).unwrap_or("");
                            let label = c.get("label").and_then(Value::as_str).unwrap_or("");
                            let mutates = c.get("mutates").and_then(Value::as_bool).unwrap_or(false);
                            mutates
                                && !id.starts_with("agent.")
                                && !id.starts_with("ui.")
                                && !id.starts_with("file.")
                                && (q.is_empty() || id.to_lowercase().contains(&q) || label.to_lowercase().contains(&q))
                        })
                        .take(400)
                        .map(|c| pick(c, &["id", "label", "params"]))
                        .collect()
                })
                .unwrap_or_default();
            Ok(Value::Array(list))
        }
        other => Err(format!("unknown tool `{other}`")),
    }
}

/// Handles one JSON-RPC message; `None` for notifications.
pub fn handle(b: &mut dyn Backend, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let ok = |r: Value| json!({"jsonrpc": "2.0", "id": id, "result": r});
    let err = |code: i64, m: String| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}});
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
            let version = if SUPPORTED.contains(&asked) { asked } else { PROTOCOL_VERSION };
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}, "experimental": {"claude/channel": {}}},
                "serverInfo": {"name": "wordcraft-agent", "title": "WordCraft agent layer", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
            match call(b, name, &args) {
                Ok(v) => ok(json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}], "isError": false})),
                Err(e) => ok(json!({"content": [{"type": "text", "text": e}], "isError": true})),
            }
        }
        "resources/list" => ok(json!({"resources": []})),
        "prompts/list" => ok(json!({"prompts": []})),
        "logging/setLevel" => ok(json!({})),
        other => err(-32601, format!("method not found: {other}")),
    })
}
