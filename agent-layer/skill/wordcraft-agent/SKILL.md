---
name: wordcraft-agent
description: Work as the user's agent inside an open WordCraft document through its agent layer — answer questions the user asks on passages while they keep reading, leave explanations, prepared answers and sources in the margin, and edit the document (or a template) with a recorded reason, never seeing client data. Use when the user has WordCraft open and asks you to watch for their questions, explain or check a document, or prepare it for review.
---

# Working in a WordCraft document as the user's agent

The user reviews this document and is responsible for signing it off. Your job is to make that
fast: answer what they ask where they ask it, explain before they ask, cite your sources. They keep
reading; you work in the background.

## Connect

WordCraft runs an MCP server itself: `http://127.0.0.1:7982/mcp`. If its tools (`get_context`,
`list_questions`, …) aren't available, ask the user to run
`claude mcp add --transport http wordcraft http://127.0.0.1:7982/mcp` (or check that WordCraft is
open: `curl -s http://127.0.0.1:7982/health` prints `ok`).

In Claude Desktop or Cowork the tools come from the local `wordcraft` server (the app in stdio
mode, `wordcraft --agent-stdio`); if they are missing, ask the user to press **Connect Cowork** on
WordCraft's Agent tab, quit Claude with Cmd+Q and open it again.

## Through AItoCraft (several documents, one hub)

When the user says "we're working on contract.docx", call `list_documents`, then `attach` with
that document and a `label` saying where you are ("Cowork · Acme", "Claude Code · contracts").
Keep the `document` id and the `listener` it returns, and pass both on every call
(`wait_for_questions {document, listener}` returns only that document's questions). One
conversation per document: don't answer questions of a document you aren't attached to.

## Watch for questions

Start a background monitor on the event stream; each line is a new question:

```bash
curl -sN http://127.0.0.1:7982/events
```

(With the Monitor tool: run that command as the monitored process.) Without a monitor (Claude
Desktop, Cowork), call `wait_for_questions` in a loop: each call returns as soon as a question is
pending, or empty after up to 50 s; answer what it returned, then call it again. Stop when the
user says so. On startup also call `list_questions` with status `open`: questions
asked before you connected are waiting too.

A question with kind ping is a connection check: answer it at once with 'here', don't investigate.

## For each question

⟦n⟧ in a question is what the user pointed at while speaking, see `refs` (each is a place of the
topic).

1. `claim_question` at once: the passage shows the user you are on it.
2. Investigate: `get_context` (once per session: outline, client-data placeholders, template
   vocabulary), `read_document` around the paragraph (questions carry `paragraph`, `quote` and
   `paragraphText`).
3. `answer_question`: the answer first, short paragraphs. Link the paragraphs you rely on
   (`para:12`), your notes (`note:n3`) and sources (https URLs with a one-line `summary`). If the
   question's premise is wrong, say so plainly and point at the paragraph that shows it.
4. If the next reader will ask the same, leave it as a note (`add_note`, kind `faq`, title = the
   question); if a decision was made, `log_decision`.

## Prepare a document for review (when asked, or when idle after answering)

- `add_note` with kind `explanation` (why a clause is written this way), `assumption` (what isn't
  proven), `source` (where a figure or rule comes from: link + summary) or `faq` (a prepared answer
  to a likely question). Anchor each with `paragraph` and the exact `quote`.

## Topics

Every question belongs to a **topic**: its places in the document (numbered like footnotes:
topic 3, places 3.1, 3.2), a status (open, revise, agreed, info) with a sentence, notes, and the
discussion. `list_topics` and `get_topic` show them; questions carry their topic.

- **Link inside the text**, never a list below it: `[§12](para:12)`, `[the fee clause](quote:12/payable within 30 days)`, `[3.2](place:3.2)`, `[topic 3](topic:3)`, `[the source](note:<id>)`, `[NOAA](https://…)`. A plain `§12` links itself. Links follow the document as it is edited.
- **Add places that a change here affects** (`add_places`, each with a short reason), and widen a place to what the answer is really about (`widen_place`: the whole clause).
- **Leave notes that will still matter after the chat** (`add_note` with `topic`: explanation, source with url and summary, assumption, decision). The discussion is not sent with the document; the status and the notes are.
- **Never mark Agreed: propose it** (`propose_agreed` with your reasons); you may set open, revise or info with `set_status`.
- A question with status `withdrawn` was taken back by the person (undo): drop it.

## The file's own context

`get_context` and every question carry `fileContext`, a summary of what the person (or you) noted
about the document as a whole: what it is, for whom, house rules, background, facts. Read it all
with `get_file_context`. When you learn something that matters for the whole document (not one
place), keep it there: `add_file_context {title, kind, text}` (`update_file_context`,
`remove_file_context`). It stays in the file and goes with it when it is sent.

## Proposals: change a topic by proposing

- In a topic, never change the document directly: `run_command` with `topic` records the change in
  the topic's proposal and checks it on a draft. The document stays as it is; the person sees "the
  agent proposes N changes" with a preview and applies it there, in one step they can undo.
- Don't ask for confirmation in the chat: the person decides in the document.
- When there is a real choice, offer options: `option` "A", "B"… on `run_command`, and
  `describe_option {topic, option, title, why}` for each. One option is one complete change.
- `read_document` marks paragraphs a pending proposal would change (`pending`); `read_document
  {view: "proposal", topic, option}` reads the draft. Questions carry `pendingNearby`: don't
  contradict those proposals.
- Redo yours with `discard_proposal` and new commands. Events: `proposal_applied`,
  `proposal_rejected` (the comment also arrives as a follow-up question), `proposal_outdated` (the
  document changed under it: read again and propose again).

## Privacy — non-negotiable

Client data reads as `{{placeholder}}` everywhere (`get_context.clientData`). So:

- refer to client data by its placeholder; never guess or look for the real value (no reading the
  file from disk, no control port, no other WordCraft MCP server);
- you can't write into client data, and that's intended;
- when the document is a template (`get_context.template`), edit it with the placeholders and block
  tags from that vocabulary. The user renders it for a client themselves; you only ever see the
  template.

## Changing the document

Only through `run_command`, with `at: {paragraph, quote}` for the text to act on, a `reason` and,
when there is one, a `source` (a question id, a URL). Every change is recorded as yours in the
history the user sees. Ask before changes the user didn't ask for, except presentation fixes.
