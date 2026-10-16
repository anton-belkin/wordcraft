use serde_json::{Value, json};
use wordcraft_doc::Document;
use wordcraft_engine::Session;

use crate::layer::QuestionStatus;

const SECRET: &str = "Hartmann Logistik GmbH";

fn letter() -> Session {
    crate::install();
    let doc = Document::from_text(&format!(
        "Engagement letter\nThis letter confirms that we act for {SECRET} in the review of its annual accounts.\nOur fee is fixed and payable in advance.\nThe engagement ends on 31 December."
    ));
    let mut s = Session::new(doc);
    s.author = "Reviewer".into();
    s
}

fn agent(s: &mut Session, id: &str, mut p: Value) -> Result<Value, String> {
    p["actor"] = json!("agent");
    s.run(id, &p).map_err(|e| e.to_string())
}

fn mark_secret(s: &mut Session) {
    s.run("agent.private", &json!({"paragraph": 1, "quote": SECRET, "label": "client.name"})).unwrap();
}

fn body(s: &Session) -> String {
    s.doc.plain_text(wordcraft_doc::StoryRef::Body)
}

#[test]
fn every_agent_read_is_masked() {
    let mut s = letter();
    mark_secret(&mut s);
    // The reader asks with the client's name in the question itself.
    s.run("agent.ask", &json!({"paragraph": 1, "quote": "annual accounts", "text": format!("Is {SECRET} really our client?")})).unwrap();
    s.run("agent.note", &json!({"paragraph": 2, "text": "Fixed fee agreed in the call."})).unwrap();
    for (id, p) in [
        ("agent.context", json!({})),
        ("agent.read", json!({})),
        ("agent.questions", json!({})),
        ("agent.layer", json!({})),
        ("agent.questions", json!({"status": "pending", "masked": false})),
        ("agent.layer", json!({"masked": false})),
    ] {
        let out = agent(&mut s, id, p).unwrap().to_string();
        assert!(!out.contains("Hartmann"), "{id} leaked client data: {out}");
        assert!(!out.contains("Logistik"), "{id} leaked client data: {out}");
    }
    let read = agent(&mut s, "agent.read", json!({})).unwrap();
    let text = read["paragraphs"][1]["text"].as_str().unwrap();
    assert_eq!(text, "This letter confirms that we act for {{client.name}} in the review of its annual accounts.");
    assert_eq!(read["paragraphs"][1]["clientData"], json!(["{{client.name}}"]));
    let qs = agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap();
    assert_eq!(qs[0]["text"], "Is {{client.name}} really our client?");
    assert_eq!(qs[0]["quote"], "annual accounts");
    assert_eq!(qs[0]["paragraph"], 1);
}

#[test]
fn the_agent_cannot_write_into_client_data() {
    let mut s = letter();
    mark_secret(&mut s);
    // Pointing at the placeholder.
    let e = agent(
        &mut s,
        "agent.exec",
        json!({"command": "text.insert", "params": {"text": "X"}, "at": {"paragraph": 1, "quote": "{{client.name}}"}, "reason": "test"}),
    );
    assert!(e.unwrap_err().contains("client data"));
    // A range that ends inside it.
    let e = agent(&mut s, "agent.exec", json!({"command": "format.bold", "at": {"paragraph": 1, "quote": "for {{client"}, "reason": "test"}));
    assert!(e.is_err());
    // A whole-paragraph change that would delete it.
    let e = agent(&mut s, "agent.exec", json!({"command": "text.delete", "at": {"paragraph": 1}, "reason": "test"}));
    assert!(e.unwrap_err().contains("client data"));
    // A replace-everything command is refused outright, and the text is unchanged.
    let e = agent(&mut s, "agent.exec", json!({"command": "document.setText", "params": {"text": "gone"}, "reason": "test"}));
    assert!(e.is_err());
    assert!(body(&s).contains(SECRET));
    // Reading or searching commands are refused (they'd be an oracle for the real text).
    for c in ["select.text", "edit.find", "edit.replaceAll", "edit.copy", "file.saveAs", "document.text"] {
        assert!(agent(&mut s, "agent.exec", json!({"command": c, "params": {"text": "Hartmann"}, "reason": "x"})).is_err(), "{c}");
    }
    // Only a person lifts the mark.
    assert!(agent(&mut s, "agent.private", json!({"paragraph": 1, "on": false})).is_err());
}

#[test]
fn the_agent_edits_around_client_data_and_is_logged() {
    let mut s = letter();
    mark_secret(&mut s);
    agent(
        &mut s,
        "agent.exec",
        json!({"command": "text.insert", "params": {"text": "audited "}, "at": {"paragraph": 1, "quote": "annual"}, "reason": "the scope covers the audited accounts", "source": "q1"}),
    )
    .unwrap();
    // text.insert replaces the selection: `annual` became `audited `.
    assert!(body(&s).contains(&format!("for {SECRET} in the review of its audited  accounts")), "{}", body(&s));
    let l = crate::layer(&s);
    let last = l.log.last().unwrap();
    assert_eq!(last.actor, crate::Actor::Agent);
    assert!(last.text.contains("the scope covers"), "{}", last.text);
    assert_eq!(last.source, "q1");
    // The client data is still found, still masked.
    let read = agent(&mut s, "agent.read", json!({})).unwrap();
    assert!(read["paragraphs"][1]["text"].as_str().unwrap().contains("{{client.name}}"));
}

#[test]
fn a_question_goes_from_pending_to_answered() {
    let mut s = letter();
    let q = s.run("agent.ask", &json!({"paragraph": 2, "quote": "payable in advance", "text": "Is that usual?"})).unwrap();
    let id = q["id"].as_str().unwrap().to_string();
    let pending = agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap();
    assert_eq!(pending.as_array().unwrap().len(), 1);
    agent(&mut s, "agent.claim", json!({"id": id})).unwrap();
    assert_eq!(crate::layer(&s).questions[0].status, QuestionStatus::Working);
    agent(&mut s, "agent.answer", json!({"id": id, "text": "Yes, for fixed fees.", "links": [{"label": "Fee policy", "href": "https://example.org/fees", "summary": "Fixed fees are billed upfront."}]})).unwrap();
    let l = crate::layer(&s);
    assert_eq!(l.questions[0].status, QuestionStatus::Answered);
    // The links are inside the text now.
    assert!(l.questions[0].answer.as_ref().unwrap().text.contains("[Fee policy](https://example.org/fees)"));
    assert!(!l.questions[0].seen);
    s.run("agent.seen", &json!({"id": id})).unwrap();
    assert!(crate::layer(&s).questions[0].seen);
}

#[test]
fn anchors_follow_edits_and_survive_undo() {
    let mut s = letter();
    s.run("agent.ask", &json!({"paragraph": 2, "quote": "payable in advance", "text": "Why?"})).unwrap();
    // A new paragraph above, and text typed before the quote.
    s.run("caret.docStart", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "DRAFT"})).unwrap();
    s.run("text.newParagraph", &json!({})).unwrap();
    s.run("select.text", &json!({"text": "Our fee"})).unwrap();
    s.run("caret.left", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "In short: "})).unwrap();
    let qs = agent(&mut s, "agent.questions", json!({})).unwrap();
    assert_eq!(qs[0]["paragraph"], 3, "{qs}");
    assert_eq!(qs[0]["quote"], "payable in advance");
    // Undo the edits: the question is still there, and found again.
    for _ in 0..3 {
        s.run("edit.undo", &json!({})).unwrap();
    }
    let qs = agent(&mut s, "agent.questions", json!({})).unwrap();
    assert_eq!(qs.as_array().unwrap().len(), 1);
    assert_eq!(qs[0]["paragraph"], 2, "{qs}");
    assert_eq!(qs[0]["quote"], "payable in advance");
}

#[test]
fn the_layer_survives_save_and_reopen() {
    let mut s = letter();
    mark_secret(&mut s);
    let q = s.run("agent.ask", &json!({"paragraph": 3, "text": "Which year?"})).unwrap();
    agent(&mut s, "agent.answer", json!({"id": q["id"], "text": "The financial year 2026."})).unwrap();
    agent(&mut s, "agent.note", json!({"paragraph": 2, "quote": "fixed", "kind": "source", "text": "Agreed fee schedule.", "links": [{"label": "Schedule", "href": "https://example.org/s", "summary": "The fee table."}]})).unwrap();
    s.run("agent.decision", &json!({"text": "Keep the fee fixed", "source": "call with the partner"})).unwrap();
    let dir = std::env::temp_dir().join(format!("wordcraft-agent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("layer.docx");
    s.run("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    // Reopened in a fresh session.
    let mut t = Session::new(Document::new());
    t.run("file.open", &json!({"path": path.to_string_lossy()})).unwrap();
    let l = crate::layer(&t);
    assert_eq!(l.questions.len(), 1);
    assert_eq!(l.questions[0].answer.as_ref().unwrap().text, "The financial year 2026.");
    assert_eq!(l.notes[0].links[0].href, "https://example.org/s");
    assert_eq!(l.private.len(), 1);
    assert!(l.log.iter().any(|e| e.text == "Keep the fee fixed"));
    // Still masked after the trip.
    let read = agent(&mut t, "agent.read", json!({})).unwrap().to_string();
    assert!(!read.contains("Hartmann") && read.contains("{{client.name}}"), "{read}");
    // And saved again: one part, not two.
    let path2 = dir.join("layer2.docx");
    t.run("file.saveAs", &json!({"path": path2.to_string_lossy()})).unwrap();
    let bytes = std::fs::read(&path2).unwrap();
    let doc = wordcraft_docx::read(&bytes).unwrap();
    let parts = doc.passthrough.values().filter(|v| std::str::from_utf8(v).is_ok_and(|t| t.contains(crate::layer::NS))).count();
    assert_eq!(parts, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn people_and_agent_edits_land_in_the_history() {
    let mut s = letter();
    s.run("agent.decision", &json!({"text": "Start review"})).unwrap();
    s.run("select.text", &json!({"text": "fixed"})).unwrap();
    s.run("format.bold", &json!({})).unwrap();
    agent(
        &mut s,
        "agent.exec",
        json!({"command": "format.italic", "at": {"paragraph": 3, "quote": "31 December"}, "reason": "dates in italics", "source": "house style"}),
    )
    .unwrap();
    let l = crate::layer(&s);
    let human = l.log.iter().find(|e| e.command == "format.bold").unwrap();
    assert_eq!(human.actor, crate::Actor::Human);
    assert_eq!(human.who, "Reviewer");
    let ag = l.log.iter().find(|e| e.command == "format.italic").unwrap();
    assert_eq!(ag.actor, crate::Actor::Agent);
    assert_eq!(ag.source, "house style");
}

#[test]
fn templates_render_per_client_and_the_agent_sees_the_template() {
    crate::install();
    let mut s = Session::new(Document::new());
    s.run("agent.openDemo", &json!({})).unwrap();
    s.run("agent.render", &json!({"client": "CL-2Q8D4R7T"})).unwrap();
    let shown = body(&s);
    assert!(shown.contains("Brightwater Shipping Ltd"));
    assert!(shown.contains("Tamsin Rook") && shown.contains("Ines Carvalho-Brandt"), "every owner listed");
    assert!(!shown.contains("{%"), "block tags are gone");
    assert!(shown.contains("three business days") && !shown.contains("named partner"), "not a priority client");
    // The agent reads the template, never the rendering.
    let read = agent(&mut s, "agent.read", json!({})).unwrap().to_string();
    assert!(read.contains("{{client.name}}") && read.contains("{% for ubo in client.ubos %}"));
    for name in ["Brightwater", "Tamsin", "Alpenglow", "GB29"] {
        assert!(!read.contains(name), "{name}");
    }
    // The agent edits the template; the rendering follows at once.
    agent(
        &mut s,
        "agent.exec",
        json!({"command": "text.insert", "params": {"text": "monthly"}, "at": {"quote": "a year"}, "reason": "billing is monthly now"}),
    )
    .unwrap();
    assert!(body(&s).contains("GBP 9,750 monthly"), "{}", body(&s));
    // Another client: re-rendered at once; the reader can't type into the preview.
    s.run("agent.render", &json!({"client": "CL-7F3K9M2Q"})).unwrap();
    assert!(body(&s).contains("Alpenglow Robotics AG") && body(&s).contains("named partner"));
    assert!(s.run("text.insert", &json!({"text": "x"})).is_err());
    // Saving saves the template.
    let dir = std::env::temp_dir().join(format!("wordcraft-agent-tpl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tpl.docx");
    s.run("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    assert!(body(&s).contains("Alpenglow"), "the preview is back after the save");
    let saved = wordcraft_docx::read(&std::fs::read(&path).unwrap()).unwrap();
    let text = saved.plain_text(wordcraft_doc::StoryRef::Body);
    assert!(text.contains("{{client.name}}") && !text.contains("Alpenglow"));
    // Back to the template.
    s.run("agent.render", &json!({"client": null})).unwrap();
    assert!(body(&s).contains("{{client.fee}} monthly"));
    // The agent can't render for a client or list them.
    assert!(agent(&mut s, "agent.render", json!({"client": "CL-7F3K9M2Q"})).is_err());
    assert!(agent(&mut s, "agent.clients", json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_in_a_preview_undoes_the_template() {
    crate::install();
    let mut s = Session::new(Document::new());
    s.run("agent.openDemo", &json!({})).unwrap();
    s.run("agent.render", &json!({"client": "CL-9V5H1N6W"})).unwrap();
    agent(&mut s, "agent.exec", json!({"command": "text.insert", "params": {"text": "ten"}, "at": {"quote": "twelve"}, "reason": "shorter term"}))
        .unwrap();
    assert!(body(&s).contains("runs for ten months"));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(body(&s).contains("runs for twelve months") && body(&s).contains("Céleste Patisserie SARL"), "{}", body(&s));
}

#[test]
fn hostile_params_never_panic() {
    let mut s = letter();
    mark_secret(&mut s);
    let junk = [
        json!(null),
        json!({}),
        json!([1, 2]),
        json!({"paragraph": -1, "quote": "", "text": "", "id": 7}),
        json!({"paragraph": 1e300, "quote": "\u{FFFC}{{", "occurrence": u64::MAX, "text": "x".repeat(50_000)}),
        json!({"paragraph": 1, "quote": "act for ", "text": "q", "links": [{"label": 1}, "x", {"label": "a", "href": ""}]}),
        json!({"command": "agent.exec", "params": {"command": "agent.exec"}, "reason": "loop"}),
        json!({"command": "text.insert", "at": {"paragraph": 99999}, "reason": "x"}),
        json!({"client": "nope"}),
        json!({"client": 5, "status": {"a": 1}, "from": "x", "count": -5}),
    ];
    let ids: Vec<&str> = s.registry.all().iter().map(|c| c.id).filter(|id| id.starts_with("agent.") && *id != "agent.openDemo").collect();
    assert!(ids.len() >= 15, "{ids:?}");
    for id in ids {
        for p in &junk {
            let _ = s.run(id, p);
            let mut q = p.clone();
            if q.is_object() {
                q["actor"] = json!("agent");
            }
            let _ = s.run(id, &q);
        }
    }
}

#[test]
fn mcp_tools_reach_the_commands() {
    let s = letter();
    let mut h = wordcraft_mcp::Headless::default();
    h.session = s;
    h.session.run("agent.private", &json!({"paragraph": 1, "quote": SECRET, "label": "client.name"})).unwrap();
    h.session.run("agent.ask", &json!({"paragraph": 1, "text": "Who signs?"})).unwrap();
    let init =
        crate::mcp::handle(&mut h, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "wordcraft-agent");
    let list = crate::mcp::handle(&mut h, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    for n in ["get_context", "list_questions", "claim_question", "answer_question", "add_note", "log_decision", "run_command", "wait_for_questions"] {
        assert!(names.contains(&n), "{n}");
    }
    let call = |h: &mut wordcraft_mcp::Headless, name: &str, args: Value| {
        let r =
            crate::mcp::handle(h, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap();
        r["result"]["content"][0]["text"].as_str().unwrap().to_string()
    };
    let pending = call(&mut h, "wait_for_questions", json!({"timeout_seconds": 0}));
    assert!(pending.contains("Who signs?") && pending.contains("{{client.name}}") && !pending.contains("Hartmann"), "{pending}");
    let event = crate::server::event_line(&serde_json::from_str::<Value>(&pending).unwrap()[0]).unwrap();
    assert_eq!(event["event"], "question");
    call(&mut h, "claim_question", json!({"id": "q3"}));
    let id = crate::layer(&h.session).questions[0].id.clone();
    call(&mut h, "claim_question", json!({"id": id}));
    call(&mut h, "answer_question", json!({"id": id, "text": "The director."}));
    assert_eq!(crate::layer(&h.session).questions[0].status, QuestionStatus::Answered);
    let doc = call(&mut h, "read_document", json!({}));
    assert!(!doc.contains("Hartmann"));
}

#[test]
fn a_connection_check_goes_out_like_a_question_but_is_never_one() {
    let mut s = letter();
    s.run("agent.ask", &json!({"paragraph": 2, "text": "A real question"})).unwrap();
    let id = s.run("agent.ping", &json!({})).unwrap()["id"].as_str().unwrap().to_string();
    // Every agent path sees it, first, with kind ping.
    let pending = agent(&mut s, "agent.questions", json!({"status": "pending", "waiting": true})).unwrap();
    assert_eq!(pending[0]["kind"], "ping");
    assert_eq!(pending[0]["id"], id);
    assert_eq!(pending.as_array().unwrap().len(), 2);
    let line = crate::server::event_line(&pending[0]).unwrap();
    assert_eq!(line["kind"], "ping");
    // Not a question in the layer: no badge, never saved.
    let l = crate::layer(&s);
    assert_eq!(l.questions.len(), 1);
    assert!(!serde_json::to_string(&*l).unwrap().contains("ping"));
    // The agent answers it: the check is answered, the real question still waits.
    agent(&mut s, "agent.claim", json!({"id": id})).unwrap();
    assert_eq!(crate::ping(&s).unwrap().status(), "working");
    agent(&mut s, "agent.answer", json!({"id": id, "text": "here"})).unwrap();
    let p = crate::ping(&s).unwrap();
    assert!(p.answered_at.is_some());
    let st = crate::presence::status(Some(&p), &[], &Default::default(), crate::now_ms());
    assert!(matches!(st, crate::presence::Status::Answering { .. }), "{st:?}");
    let pending = agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap();
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["text"], "A real question");
    // The agent can't send a check itself.
    assert!(agent(&mut s, "agent.ping", json!({})).is_err());
    // A wait_for_questions poll counts as a listener, and isn't a "tool call".
    assert!(crate::presence::listeners(false).waiting);
}

#[test]
fn aitocraft_documents_attach_and_listeners() {
    let mut s = letter();
    s.file_name = Some("contract.docx".into());
    let me = crate::document_id(&s);
    assert!(me.starts_with(&format!("wordcraft:{}:", std::process::id())));
    // list_documents: the registry's object.
    let docs = agent(&mut s, "agent.documents", json!({})).unwrap();
    assert_eq!(docs[0]["id"], me.as_str());
    assert_eq!(docs[0]["title"], "contract.docx");
    assert_eq!(docs[0]["frontmost"], true);
    // A document that isn't open here is an error, by name.
    let e = agent(&mut s, "agent.context", json!({"document": "wordcraft:1:999"})).unwrap_err();
    assert!(e.contains("document wordcraft:1:999 is not open in WordCraft"), "{e}");
    assert!(agent(&mut s, "agent.read", json!({"document": me})).is_ok());
    // attach → a listener; questions carry their document; a call with the listener keeps it.
    let a = agent(&mut s, "agent.attach", json!({"document": me, "label": "Cowork · Acme"})).unwrap();
    let l = a["listener"].as_str().unwrap().to_string();
    assert_eq!(a["document"], me.as_str());
    assert_eq!(a["title"], "contract.docx");
    assert_eq!(a["app"], "wordcraft");
    s.run("agent.ask", &json!({"paragraph": 2, "text": "Fixed fee?"})).unwrap();
    let qs = agent(&mut s, "agent.questions", json!({"status": "pending", "document": me, "listener": l})).unwrap();
    assert_eq!(qs[0]["document"], me.as_str());
    assert_eq!(qs[0]["documentTitle"], "contract.docx");
    assert!(crate::server::for_document(&qs[0], Some(&me)));
    assert!(!crate::server::for_document(&qs[0], Some("gridcraft:1:1")));
    assert_eq!(crate::document_json(&s)["listeners"][0]["label"], "Cowork · Acme");
    // The ping goes to this document's listeners and records who answered.
    let ping = s.run("agent.ping", &json!({})).unwrap()["id"].as_str().unwrap().to_string();
    agent(&mut s, "agent.answer", json!({"id": ping, "text": "here", "listener": l})).unwrap();
    assert_eq!(crate::ping(&s).unwrap().answered_by, "Cowork · Acme");
    // detach; an unknown listener must attach again.
    assert_eq!(agent(&mut s, "agent.detach", json!({"listener": l})).unwrap()["detached"], true);
    let e = agent(&mut s, "agent.questions", json!({"listener": l})).unwrap_err();
    assert!(e.contains("attach again"), "{e}");
    assert_eq!(crate::server::query_param("document=wordcraft%3A12%3A3&x=1", "document").as_deref(), Some("wordcraft:12:3"));
}

#[test]
fn every_mcp_tool_takes_a_document() {
    for t in crate::mcp::tools() {
        let name = t["name"].as_str().unwrap();
        assert!(t["inputSchema"]["properties"]["document"].is_object(), "{name}");
        let has_listener = t["inputSchema"]["properties"]["listener"].is_object();
        assert_eq!(has_listener, ["list_questions", "wait_for_questions", "claim_question", "answer_question", "detach"].contains(&name), "{name}");
    }
    let names: Vec<String> = crate::mcp::tools().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    for n in ["list_documents", "attach", "detach"] {
        assert!(names.iter().any(|x| x == n), "{n}");
    }
}

// ---------------------------------------------------------------- topics (TOPICS.md)

fn topics_of(s: &mut Session) -> Value {
    agent(s, "agent.topics", json!({})).unwrap()
}

fn select(s: &mut Session, text: &str) {
    s.run("select.text", &json!({"text": text})).unwrap();
}

#[test]
fn a_question_makes_a_topic_with_its_selection_as_a_bookmarked_place() {
    let mut s = letter();
    select(&mut s, "payable in advance");
    let r = s.run("agent.ask", &json!({"text": "Is that usual?"})).unwrap();
    assert_eq!(r["newTopic"], true);
    let t = topics_of(&mut s);
    assert_eq!(t["topics"][0]["topic"], 1);
    assert_eq!(t["topics"][0]["status"], "open");
    assert_eq!(t["topics"][0]["places"][0]["place"], "1");
    assert_eq!(t["topics"][0]["places"][0]["quote"], "payable in advance");
    // A bookmark in the document, hidden (its name starts with "_").
    let m = crate::places::scan(&s.doc);
    assert_eq!(m.len(), 1);
    assert!(m.keys().next().unwrap().starts_with("_GcPlace_"));
    // Asking again on the same text adds to the topic; "New topic" starts another.
    select(&mut s, "payable in advance");
    assert_eq!(s.run("agent.ask", &json!({"text": "And for whom?"})).unwrap()["newTopic"], false);
    select(&mut s, "payable in advance");
    assert_eq!(s.run("agent.ask", &json!({"text": "Separate", "newTopic": true})).unwrap()["newTopic"], true);
    assert_eq!(topics_of(&mut s)["topics"].as_array().unwrap().len(), 2);
    // Questions carry their topic.
    let qs = agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap();
    assert_eq!(qs[1]["topic"]["number"], 1);
    assert_eq!(qs[1]["topic"]["status"], "open");
}

#[test]
fn copy_and_paste_duplicates_a_place_and_cut_moves_it() {
    let mut s = letter();
    select(&mut s, "payable in advance");
    s.run("agent.ask", &json!({"text": "Is that usual?"})).unwrap();
    // Copy, paste at the end of the document: the topic gains a place.
    select(&mut s, "payable in advance");
    s.run("edit.copy", &json!({})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("edit.paste", &json!({})).unwrap();
    let t = topics_of(&mut s);
    let places = t["topics"][0]["places"].as_array().unwrap().clone();
    assert_eq!(places.len(), 2, "{t}");
    assert_eq!(places[0]["place"], "1.1");
    assert_eq!(places[1]["place"], "1.2");
    assert_eq!(places[1]["paragraph"], 3);
    assert_eq!(crate::places::scan(&s.doc).len(), 2, "two bookmarks, two names");
    // Cut and paste moves a place: still one place, now where the text went.
    let mut s = letter();
    select(&mut s, "fixed");
    s.run("agent.ask", &json!({"text": "Why fixed?"})).unwrap();
    select(&mut s, "fixed");
    s.run("edit.cut", &json!({})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("edit.paste", &json!({})).unwrap();
    let t = topics_of(&mut s);
    let places = t["topics"][0]["places"].as_array().unwrap().clone();
    assert_eq!(places.len(), 1, "{t}");
    assert_eq!(places[0]["paragraph"], 3);
    assert!(places[0].get("removed").is_none(), "{t}");
    assert_eq!(crate::places::scan(&s.doc).len(), 1);
}

#[test]
fn deleted_text_keeps_its_whereabouts_and_undo_brings_it_back() {
    let mut s = letter();
    select(&mut s, "payable in advance");
    s.run("agent.ask", &json!({"text": "Is that usual?"})).unwrap();
    select(&mut s, "payable in advance");
    s.run("text.delete", &json!({})).unwrap();
    assert!(!body(&s).contains("payable in advance"));
    let t = topics_of(&mut s);
    let p = &t["topics"][0]["places"][0];
    assert_eq!(p["removed"], true, "{t}");
    assert_eq!(p["quote"], "payable in advance");
    assert_eq!(p["paragraph"], 2);
    assert!(crate::places::scan(&s.doc).values().next().unwrap()[0].collapsed());
    s.run("edit.undo", &json!({})).unwrap();
    let t = topics_of(&mut s);
    assert!(t["topics"][0]["places"][0].get("removed").is_none(), "{t}");
    assert_eq!(t["topics"][0]["places"][0]["quote"], "payable in advance");
}

#[test]
fn undo_of_a_question_withdraws_it_and_redo_sends_it_again() {
    let mut s = letter();
    select(&mut s, "payable in advance");
    let id = s.run("agent.ask", &json!({"text": "Is that usual?"})).unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap().as_array().unwrap().len(), 1);
    // Not taken yet: the question and its place go.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap().as_array().unwrap().is_empty());
    assert!(topics_of(&mut s)["topics"].as_array().unwrap().is_empty());
    assert!(crate::places::scan(&s.doc).is_empty());
    // Redo: back, and sent again.
    s.run("edit.redo", &json!({})).unwrap();
    let qs = agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap();
    assert_eq!(qs[0]["id"], id.as_str());
    assert_eq!(crate::layer(&s).questions[0].round, 1);
    assert_eq!(topics_of(&mut s)["topics"].as_array().unwrap().len(), 1);
    // Being worked on, then undone: withdrawn, and the agent's answer is refused.
    agent(&mut s, "agent.claim", json!({"id": id})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(agent(&mut s, "agent.questions", json!({"status": "withdrawn"})).unwrap().as_array().unwrap().len(), 1);
    let e = agent(&mut s, "agent.answer", json!({"id": id, "text": "x"})).unwrap_err();
    assert!(e.contains("withdrawn by the user; drop it"), "{e}");
    // Answered, undone, redone: back with its answer, not asked again.
    s.run("edit.redo", &json!({})).unwrap();
    agent(&mut s, "agent.claim", json!({"id": id})).unwrap();
    agent(&mut s, "agent.answer", json!({"id": id, "text": "Yes."})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(topics_of(&mut s)["topics"].as_array().unwrap().is_empty());
    s.run("edit.redo", &json!({})).unwrap();
    let q = crate::layer(&s).questions[0].clone();
    assert_eq!(q.status, QuestionStatus::Answered);
    assert!(agent(&mut s, "agent.questions", json!({"status": "pending"})).unwrap().as_array().unwrap().is_empty());
}

#[test]
fn only_a_person_sets_agreed_and_status_changes_undo() {
    let mut s = letter();
    select(&mut s, "fixed");
    s.run("agent.ask", &json!({"text": "Fixed?"})).unwrap();
    let e = agent(&mut s, "agent.topicStatus", json!({"topic": 1, "status": "agreed", "text": "fine"})).unwrap_err();
    assert!(e.contains("only a person sets Agreed"), "{e}");
    agent(&mut s, "agent.topicStatus", json!({"topic": 1, "status": "revise", "text": "the fee must be per year, see §2"})).unwrap();
    agent(&mut s, "agent.proposeAgreed", json!({"topic": 1, "reasons": "the partner confirmed it"})).unwrap();
    let t = topics_of(&mut s);
    assert_eq!(t["topics"][0]["status"], "revise");
    assert_eq!(t["topics"][0]["proposal"]["status"], "agreed");
    assert!(t["topics"][0]["statusText"].as_str().unwrap().contains("[§2](para:2)"), "{t}");
    // The person confirms with one click; ⌘Z takes it back.
    s.run("agent.confirmAgreed", &json!({"topic": 1})).unwrap();
    assert_eq!(topics_of(&mut s)["topics"][0]["status"], "agreed");
    assert_eq!(topics_of(&mut s)["signOff"]["line"], "Agreed 1 of 1 · 0 revise · 0 open");
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(topics_of(&mut s)["topics"][0]["status"], "revise");
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(topics_of(&mut s)["topics"][0]["status"], "agreed");
    // Every change is in the history, with who.
    let l = crate::layer(&s);
    assert!(l.log.iter().any(|e| e.kind == "status" && e.actor == crate::Actor::Agent));
    assert!(l.log.iter().any(|e| e.kind == "status" && e.actor == crate::Actor::Human && e.text.contains("Agreed")));
}

#[test]
fn links_in_text_follow_the_document() {
    let mut s = letter();
    select(&mut s, "fixed");
    let id = s.run("agent.ask", &json!({"text": "Fixed?"})).unwrap()["id"].as_str().unwrap().to_string();
    agent(&mut s, "agent.answer", json!({"id": id, "text": "See §3 and [the end](quote:3/31 December)."})).unwrap();
    // A paragraph above: §3 is now §4, the quote still leads to its text.
    s.run("caret.docStart", &json!({})).unwrap();
    s.run("text.newParagraph", &json!({})).unwrap();
    let qs = agent(&mut s, "agent.questions", json!({})).unwrap();
    let a = qs[0]["answer"]["text"].as_str().unwrap();
    assert!(a.contains("[§4](para:4)") && a.contains("[the end](para:4)"), "{a}");
}

#[test]
fn the_agents_places_stay_when_the_person_undoes() {
    let mut s = letter();
    select(&mut s, "fixed");
    s.run("agent.ask", &json!({"text": "Fixed?"})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": " Signed."})).unwrap();
    agent(
        &mut s,
        "agent.addPlaces",
        json!({"topic": 1, "places": [{"paragraph": 3, "quote": "31 December", "reason": "the term ends with the fee period"}]}),
    )
    .unwrap();
    assert_eq!(topics_of(&mut s)["topics"][0]["places"].as_array().unwrap().len(), 2);
    // The person undoes their typing (before the agent's place existed): the place stays.
    s.run("edit.undo", &json!({})).unwrap();
    let t = topics_of(&mut s);
    let ps = t["topics"][0]["places"].as_array().unwrap();
    assert_eq!(ps.len(), 2, "{t}");
    assert_eq!(ps[1]["reason"], "the term ends with the fee period");
    assert_eq!(ps[1]["quote"], "31 December");
    // widen_place: the whole paragraph.
    agent(&mut s, "agent.widenPlace", json!({"place": "1.2", "to": {"paragraph": 3}, "reason": "the whole clause"})).unwrap();
    assert_eq!(topics_of(&mut s)["topics"][0]["places"][1]["quote"], "The engagement ends on 31 December.");
    // The person rejects it (undoable).
    s.run("agent.rejectPlace", &json!({"place": "1.2"})).unwrap();
    assert_eq!(topics_of(&mut s)["topics"][0]["places"].as_array().unwrap().len(), 1);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(topics_of(&mut s)["topics"][0]["places"].as_array().unwrap().len(), 2);
}

#[test]
fn sending_drops_the_discussion_unless_included() {
    let mut s = letter();
    select(&mut s, "fixed");
    let id = s.run("agent.ask", &json!({"text": "Fixed?"})).unwrap()["id"].as_str().unwrap().to_string();
    agent(&mut s, "agent.answer", json!({"id": id, "text": "Yes."})).unwrap();
    agent(&mut s, "agent.note", json!({"topic": 1, "title": "Fee policy", "kind": "decision", "text": "Fixed fee, agreed."})).unwrap();
    s.run("agent.setStatus", &json!({"topic": 1, "status": "agreed", "text": "with the partner"})).unwrap();
    let dir = std::env::temp_dir().join(format!("wordcraft-send-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, include) in [("plain.docx", false), ("with.docx", true)] {
        let path = dir.join(name);
        s.run("agent.saveForSending", &json!({"path": path.to_string_lossy(), "includeDiscussion": include})).unwrap();
        let doc = wordcraft_docx::read(&std::fs::read(&path).unwrap()).unwrap();
        let l = crate::layer::read_from(&doc);
        assert_eq!(l.topics.len(), 1);
        assert_eq!(l.topics[0].status.kind, crate::layer::StatusKind::Agreed);
        assert_eq!(l.notes.len(), 1);
        assert_eq!(l.questions.len(), if include { 1 } else { 0 }, "{name}");
        assert_eq!(l.log.iter().any(|e| e.text.contains("Fixed?")), include, "{name}");
        // The places' bookmarks travel with the file.
        assert_eq!(crate::places::scan(&doc).len(), 1);
    }
    // The document itself is unchanged (still its own path, its discussion).
    assert_eq!(crate::layer(&s).questions.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn version_1_files_become_topics() {
    crate::install();
    let mut doc = Document::from_text("Intro\nOur fee is fixed and payable in advance.\nEnd");
    let v1 = json!({
        "version": 0, "next_id": 3,
        "anchors": [{"id": "a1", "path": [1], "start": 11, "end": 16, "quote": "fixed"}, {"id": "a2", "path": [2], "start": 0, "end": 3, "quote": "End"}],
        "questions": [{"id": "q1", "anchor": "a1", "text": "Why fixed?", "status": "answered", "seen": true, "answer": {"text": "Policy."}}],
        "notes": [{"id": "n2", "anchor": "a2", "kind": "source", "text": "See the policy."}]
    });
    let xml = format!("<?xml version=\"1.0\"?><agentLayer xmlns=\"{}\"><![CDATA[{}]]></agentLayer>", crate::layer::NS, v1);
    doc.passthrough.insert("customXml/item9.xml".into(), std::sync::Arc::new(xml.into_bytes()));
    let mut s = Session::new(doc);
    let l = crate::layer(&s);
    assert_eq!(l.version, crate::layer::VERSION);
    assert_eq!(l.topics.len(), 2);
    assert_eq!(l.questions[0].topic, l.topics[0].id);
    assert_eq!(l.topics[0].status.kind, crate::layer::StatusKind::Info, "answered and read");
    assert_eq!(l.notes[0].topic, l.topics[1].id);
    // The first edit gives the migrated places their bookmarks.
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "."})).unwrap();
    assert_eq!(crate::places::scan(&s.doc).len(), 2);
    let t = agent(&mut s, "agent.topics", json!({})).unwrap();
    assert_eq!(t["topics"][0]["places"][0]["quote"], "fixed");
}
