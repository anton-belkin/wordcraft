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

## Watch for questions

Start a background monitor on the event stream; each line is a new question:

```bash
curl -sN http://127.0.0.1:7982/events
```

(With the Monitor tool: run that command as the monitored process.) Without a monitor, call
`wait_for_questions` in a loop. On startup also call `list_questions` with status `open`: questions
asked before you connected are waiting too.

## For each question

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
