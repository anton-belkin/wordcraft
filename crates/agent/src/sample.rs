//! The demo document: an engagement letter written as a template (our own text, invented firm and
//! terms). It has inline placeholders, a loop over the client's beneficial owners and a condition,
//! so the reader can switch clients and watch it re-render while the agent edits the template.

use std::sync::Arc;

use wordcraft_doc::props::CharProps;
use wordcraft_doc::{Block, Document, Paragraph};

fn p(text: &str) -> Arc<Block> {
    Arc::new(Block::Para(Paragraph::with_text(text, CharProps::default())))
}

fn h(text: &str, style: &str) -> Arc<Block> {
    Arc::new(Block::Para(Paragraph::with_text(text, CharProps::default()).styled(style)))
}

/// The engagement letter template.
pub fn engagement_letter() -> Document {
    let mut d = Document::new();
    d.core.title = "Engagement letter (template)".into();
    d.core.creator = "WordCraft agent layer demo".into();
    d.body = vec![
        h("Engagement letter", "Title"),
        p("Studio Ledger Advisory · Rue du Marché 12 · 1204 Geneva"),
        p("Internal reference KYC-2026-0412 · relationship manager: Dana Whitcombe"),
        p("To {{client.name}}, {{client.legal_form}}, {{client.address}}, {{client.country}}. For the attention of {{client.director}}."),
        h("1. Scope of the engagement", "Heading1"),
        p(
            "We will prepare the annual compliance file of {{client.name}}: the review of its corporate records, the beneficial ownership register and the anti-money-laundering checks required for a company registered in {{client.country}}.",
        ),
        p(
            "The work is done on the basis of the documents the client provides. We do not audit the financial statements and give no assurance on them.",
        ),
        h("2. Beneficial owners on file", "Heading1"),
        p(
            "The register we hold lists the following beneficial owners; each one is checked against public sanctions lists before the file is signed:",
        ),
        p("{% for ubo in client.ubos %}"),
        p("{{ubo.name}} ({{ubo.nationality}}), holding {{ubo.share}} of the shares."),
        p("{% endfor %}"),
        h("3. Fees", "Heading1"),
        p(
            "Our fee for the engagement is {{client.fee}} a year, invoiced in advance and payable within 30 days to the account the client names, currently {{client.iban}}.",
        ),
        p("{% if client.vip %}"),
        p("As a priority client, {{client.name}} has a named partner and a response within one business day."),
        p("{% else %}"),
        p("Questions are answered within three business days by the team on duty."),
        p("{% endif %}"),
        h("4. Term and termination", "Heading1"),
        p(
            "The engagement runs for twelve months from signature and renews for the same period unless either party gives three months' written notice. Fees for work already done remain due.",
        ),
        p(
            "Data the client provides is kept for ten years after the end of the engagement, as the anti-money-laundering rules require, and then deleted.",
        ),
        h("Signatures", "Heading2"),
        p("For Studio Ledger Advisory: ______________________"),
        p("For {{client.name}}: {{client.director}} ______________________"),
    ];
    d
}
