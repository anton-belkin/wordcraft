//! A local "CRM" for template demos: three invented clients, built in code (no real data, nothing
//! read from anywhere). The reader picks one and the template renders with its data; the agent
//! never sees these values, only the placeholders' names.

use serde_json::{Value as Json, json};

/// The clients, as the template engine reads them: `{"client": {...}}` per client.
pub fn clients() -> Vec<Json> {
    vec![
        json!({
            "id": "CL-7F3K9M2Q",
            "name": "Alpenglow Robotics AG",
            "legal_form": "Aktiengesellschaft",
            "address": "Seestrasse 41, 8802 Kilchberg",
            "country": "Switzerland",
            "director": "Mirela Haldimann",
            "email": "office@alpenglow-robotics.example",
            "iban": "CH93 0076 2011 6238 5295 7",
            "fee": "CHF 18,400",
            "vip": true,
            "ubos": [
                {"name": "Mirela Haldimann", "share": "62%", "nationality": "Swiss"},
                {"name": "Joris Vanterpool", "share": "38%", "nationality": "Dutch"}
            ]
        }),
        json!({
            "id": "CL-2Q8D4R7T",
            "name": "Brightwater Shipping Ltd",
            "legal_form": "private limited company",
            "address": "14 Harbour Row, Falmouth TR11 3XB",
            "country": "United Kingdom",
            "director": "Oluwaseun Pemberton",
            "email": "accounts@brightwater-shipping.example",
            "iban": "GB29 NWBK 6016 1331 9268 19",
            "fee": "GBP 9,750",
            "vip": false,
            "ubos": [
                {"name": "Oluwaseun Pemberton", "share": "40%", "nationality": "British"},
                {"name": "Ines Carvalho-Brandt", "share": "35%", "nationality": "Portuguese"},
                {"name": "Tamsin Rook", "share": "25%", "nationality": "British"}
            ]
        }),
        json!({
            "id": "CL-9V5H1N6W",
            "name": "Céleste Patisserie SARL",
            "legal_form": "société à responsabilité limitée",
            "address": "7 rue des Glycines, 74100 Annemasse",
            "country": "France",
            "director": "Anaïs Lefebvre-Okafor",
            "email": "bonjour@celeste-patisserie.example",
            "iban": "FR76 3000 6000 0112 3456 7890 189",
            "fee": "EUR 4,200",
            "vip": false,
            "ubos": [
                {"name": "Anaïs Lefebvre-Okafor", "share": "100%", "nationality": "French"}
            ]
        }),
    ]
}

/// The client with this id.
pub fn client(id: &str) -> Option<Json> {
    clients().into_iter().find(|c| c.get("id").and_then(Json::as_str) == Some(id))
}

/// The template vocabulary, as the agent may know it: placeholder names, never values.
pub fn vocabulary() -> Json {
    json!({
        "placeholders": ["client.name", "client.legal_form", "client.address", "client.country", "client.director", "client.email", "client.iban", "client.fee"],
        "conditions": ["client.vip"],
        "lists": {"client.ubos": ["name", "share", "nationality"]},
        "syntax": "Inline: {{client.name}}. Blocks, each tag alone in its own paragraph: {% for ubo in client.ubos %} … {{ubo.name}} … {% endfor %}; {% if client.vip %} … {% else %} … {% endif %}.",
    })
}

/// Every identifying value of every client as (value, placeholder) pairs: what must never reach
/// the agent. Generic words (a country, a legal form, a share) are left out: scrubbing them would
/// mangle ordinary text, and they don't say who the client is.
pub fn secrets() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for c in clients() {
        let Some(o) = c.as_object() else { continue };
        for (k, v) in o {
            if matches!(k.as_str(), "id" | "vip" | "country" | "legal_form") {
                continue;
            }
            match v {
                Json::String(s) => out.push((s.clone(), crate::mask::placeholder(&format!("client.{k}")))),
                Json::Array(items) => {
                    for item in items {
                        if let Some(s) = item.get("name").and_then(Json::as_str) {
                            out.push((s.to_string(), crate::mask::placeholder("ubo.name")));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}
