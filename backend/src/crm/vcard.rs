//! Reading contacts from vCard files (versions 2.1, 3.0 and 4.0), as phones, mail programs
//! and address books export them, into CRM contact bodies.

use encoding_rs::Encoding;
use serde_json::{json, Map, Value};

/// One card, ready to become a contact: its body (without the company link) and the company
/// name from `ORG`, if any.
#[derive(Debug, Default)]
pub struct Card {
    /// The card's first line in the file (1-based), for error reports.
    pub line: usize,
    pub body: Map<String, Value>,
    pub company: Option<String>,
}

/// Text of an uploaded file: UTF-8 (with or without a byte-order mark), UTF-16 with a mark,
/// or else Windows-1252, which older exports from mail programs use.
pub fn decode(bytes: &[u8]) -> String {
    if let Some((enc, bom)) = Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom..]).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

/// Lines with folding undone (a line starting with a space or tab continues the one before;
/// in 2.1 quoted-printable values a trailing `=` continues onto the next line).
fn unfold(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut qp_open = false;
    for (n, raw) in text.split('\n').enumerate() {
        let l = raw.strip_suffix('\r').unwrap_or(raw);
        if qp_open && let Some(last) = out.last_mut() {
            last.1.pop(); // the soft break's '='
            last.1.push_str(l);
            qp_open = last.1.ends_with('=') && is_qp(&last.1);
            continue;
        }
        if (l.starts_with(' ') || l.starts_with('\t')) && let Some(last) = out.last_mut() {
            last.1.push_str(&l[1..]);
            continue;
        }
        if l.trim().is_empty() {
            continue;
        }
        out.push((n + 1, l.to_string()));
        qp_open = l.ends_with('=') && is_qp(l);
    }
    out
}

fn is_qp(line: &str) -> bool {
    line.split(':').next().is_some_and(|h| h.to_ascii_uppercase().contains("QUOTED-PRINTABLE"))
}

fn qp_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' {
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(|c| (*c as char).to_digit(16)), b.get(i + 2).and_then(|c| (*c as char).to_digit(16))) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Splits on unescaped `sep`, undoing `\n`, `\,`, `\;` and `\\`.
fn split_escaped(s: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n' | 'N') => parts.last_mut().unwrap().push('\n'),
                Some(x) => parts.last_mut().unwrap().push(x),
                None => {}
            },
            c if c == sep => parts.push(String::new()),
            c => parts.last_mut().unwrap().push(c),
        }
    }
    parts.into_iter().map(|p| p.trim().to_string()).collect()
}

struct Prop {
    name: String,
    /// TYPE values, lower-case (from TYPE=a,b and 2.1's bare parameters).
    types: Vec<String>,
    value: String,
}

fn parse_prop(line: &str) -> Option<Prop> {
    // The value starts at the first ':' outside a quoted parameter.
    let mut in_quote = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, raw) = (&line[..colon], &line[colon + 1..]);
    let mut params = head.split(';');
    let name = params.next()?.rsplit('.').next()?.to_ascii_uppercase(); // "item1.EMAIL" → EMAIL
    let mut types = Vec::new();
    let (mut qp, mut charset) = (false, None::<String>);
    for p in params {
        let (k, v) = p.split_once('=').map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"'))).unwrap_or(("TYPE".into(), p));
        match k.as_str() {
            "TYPE" => types.extend(v.split(',').map(|t| t.trim().to_ascii_lowercase())),
            "ENCODING" if v.eq_ignore_ascii_case("QUOTED-PRINTABLE") => qp = true,
            "CHARSET" => charset = Some(v.to_string()),
            "PREF" => types.push("pref".into()),
            _ => {}
        }
    }
    let value = if qp {
        let bytes = qp_decode(raw);
        let enc = charset.as_deref().and_then(|c| Encoding::for_label(c.as_bytes())).unwrap_or(encoding_rs::UTF_8);
        enc.decode_without_bom_handling(&bytes).0.into_owned()
    } else {
        raw.to_string()
    };
    Some(Prop { name, types, value })
}

fn phone_type(types: &[String]) -> &'static str {
    if types.iter().any(|t| t == "cell" || t == "mobile" || t == "iphone") {
        "mobile"
    } else if types.iter().any(|t| t == "fax") {
        "fax"
    } else if types.iter().any(|t| t == "home") {
        "home"
    } else if types.iter().any(|t| t == "work") {
        "work"
    } else {
        "other"
    }
}

fn email_type(types: &[String]) -> &'static str {
    if types.iter().any(|t| t == "home") {
        "home"
    } else if types.iter().any(|t| t == "work") {
        "work"
    } else {
        "other"
    }
}

/// Instant messengers from IMPP / X-… properties.
fn messenger(name: &str, value: &str, types: &[String]) -> Option<(&'static str, String)> {
    const KNOWN: &[&str] = &["telegram", "whatsapp", "signal", "viber", "wechat", "line", "skype"];
    let lower = value.to_ascii_lowercase();
    let scheme = lower.split(':').next().unwrap_or("");
    let handle = value.split_once(':').map(|(_, h)| h).unwrap_or(value).trim().to_string();
    let named = KNOWN.iter().find(|k| scheme == **k || types.iter().any(|t| t == *k) || name == format!("X-{}", k.to_ascii_uppercase()));
    match (name, named) {
        (_, Some(k)) => Some((k, handle)),
        ("IMPP", None) => Some(("other", handle)),
        _ => None,
    }
}

fn birthdate(v: &str) -> Option<String> {
    let d: String = v.chars().take_while(|c| *c != 'T').filter(|c| c.is_ascii_digit()).collect();
    (d.len() == 8 && !v.starts_with("--")).then(|| format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]))
}

/// Reads every card of a file. Cards without a name, phone or email are dropped.
pub fn parse(text: &str) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut cur: Option<(Card, Vec<Value>, Option<String>)> = None;
    for (line, l) in unfold(text) {
        let Some(p) = parse_prop(&l) else { continue };
        match p.name.as_str() {
            "BEGIN" if p.value.eq_ignore_ascii_case("VCARD") => cur = Some((Card { line, ..Default::default() }, Vec::new(), None)),
            "END" if p.value.eq_ignore_ascii_case("VCARD") => {
                if let Some((mut c, multi, full)) = cur.take() {
                    if !c.body.contains_key("first_name") && !c.body.contains_key("last_name") {
                        // Only FN: "First Last" (or an organisation's name for company cards).
                        if let Some(f) = full.filter(|f| !f.is_empty()) {
                            match f.rsplit_once(' ') {
                                Some((first, last)) => {
                                    c.body.insert("first_name".into(), json!(first.trim()));
                                    c.body.insert("last_name".into(), json!(last.trim()));
                                }
                                None => {
                                    c.body.insert("first_name".into(), json!(f));
                                }
                            }
                        } else if let Some(org) = &c.company {
                            c.body.insert("first_name".into(), json!(org));
                        }
                    }
                    if !multi.is_empty() {
                        c.body.insert("multifields".into(), Value::Array(multi));
                    }
                    if c.body.contains_key("first_name") || c.body.contains_key("last_name") || c.body.contains_key("multifields") {
                        cards.push(c);
                    }
                }
            }
            _ => {
                let Some((c, multi, full)) = cur.as_mut() else { continue };
                let one = || split_escaped(&p.value, '\u{0}').concat();
                match p.name.as_str() {
                    "N" => {
                        let n = split_escaped(&p.value, ';');
                        let get = |i: usize| n.get(i).map(|s| s.replace(',', " ").trim().to_string()).filter(|s| !s.is_empty());
                        for (i, k) in [(0, "last_name"), (1, "first_name"), (2, "middle_name"), (3, "honorific")] {
                            if let Some(v) = get(i) {
                                c.body.insert(k.into(), json!(v.chars().take(100).collect::<String>()));
                            }
                        }
                    }
                    "FN" => *full = Some(one()),
                    "ORG" => c.company = split_escaped(&p.value, ';').into_iter().next().filter(|s| !s.is_empty()),
                    "TITLE" => {
                        c.body.insert("job_title".into(), json!(one()));
                    }
                    "TEL" => {
                        let v = one();
                        let v = v.strip_prefix("tel:").unwrap_or(&v).to_string();
                        if !v.is_empty() {
                            multi.push(json!({ "kind": "phone", "value_type": phone_type(&p.types), "value": v }));
                        }
                    }
                    "EMAIL" => {
                        let v = one();
                        if v.contains('@') {
                            multi.push(json!({ "kind": "email", "value_type": email_type(&p.types), "value": v }));
                        }
                    }
                    "URL" => {
                        let v = one();
                        if v.starts_with("http://") || v.starts_with("https://") {
                            multi.push(json!({ "kind": "web", "value_type": if p.types.iter().any(|t| t == "home") { "personal" } else { "work" }, "value": v }));
                        }
                    }
                    "ADR" if !c.body.contains_key("address") => {
                        let a = split_escaped(&p.value, ';');
                        let mut m = Map::new();
                        let street = [a.get(1), a.get(2)].into_iter().flatten().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>();
                        if let Some(s) = street.first() {
                            m.insert("line1".into(), json!(s.replace('\n', ", ")));
                        }
                        if let Some(s) = street.get(1) {
                            m.insert("line2".into(), json!(s));
                        }
                        for (i, k) in [(3, "city"), (4, "region"), (5, "postal_code")] {
                            if let Some(v) = a.get(i).filter(|v| !v.is_empty()) {
                                m.insert(k.into(), json!(v));
                            }
                        }
                        // Only ISO codes are kept; country names vary too much to guess.
                        if let Some(cc) = a.get(6).filter(|v| v.len() == 2 && v.chars().all(|c| c.is_ascii_alphabetic())) {
                            m.insert("country".into(), json!(cc.to_ascii_uppercase()));
                        }
                        if !m.is_empty() {
                            c.body.insert("address".into(), Value::Object(m));
                        }
                    }
                    "BDAY" => {
                        if let Some(d) = birthdate(&p.value) {
                            c.body.insert("birthdate".into(), json!(d));
                        }
                    }
                    "NOTE" => {
                        c.body.insert("comments".into(), json!(one()));
                    }
                    name => {
                        if let Some((kind, handle)) = messenger(name, &p.value, &p.types).filter(|(_, h)| !h.is_empty()) {
                            multi.push(json!({ "kind": "im", "value_type": kind, "value": handle }));
                        }
                    }
                }
            }
        }
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_3() {
        let cards = parse(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Doe;Jane;Q.;Dr.;\r\nFN:Dr. Jane Q. Doe\r\nORG:Acme\\, Inc.;Sales\r\nTITLE:Buyer\r\n\
             TEL;TYPE=CELL:+1 555 0100\r\nTEL;TYPE=WORK,VOICE:+1 555 0101\r\nitem1.EMAIL;TYPE=INTERNET:jane@acme.example\r\n\
             ADR;TYPE=WORK:;;1 Main St;Springfield;IL;62701;US\r\nBDAY:1980-04-02\r\nNOTE:Met at the fair\\nlikes tea\r\n\
             URL:https://acme.example\r\nIMPP:telegram:janedoe\r\nEND:VCARD\r\n",
        );
        assert_eq!(cards.len(), 1);
        let c = &cards[0];
        assert_eq!(c.body["last_name"], "Doe");
        assert_eq!(c.body["first_name"], "Jane");
        assert_eq!(c.body["middle_name"], "Q.");
        assert_eq!(c.body["honorific"], "Dr.");
        assert_eq!(c.company.as_deref(), Some("Acme, Inc."));
        assert_eq!(c.body["job_title"], "Buyer");
        assert_eq!(c.body["birthdate"], "1980-04-02");
        assert_eq!(c.body["comments"], "Met at the fair\nlikes tea");
        assert_eq!(c.body["address"]["city"], "Springfield");
        assert_eq!(c.body["address"]["country"], "US");
        let m = c.body["multifields"].as_array().unwrap();
        assert!(m.contains(&json!({ "kind": "phone", "value_type": "mobile", "value": "+1 555 0100" })));
        assert!(m.contains(&json!({ "kind": "phone", "value_type": "work", "value": "+1 555 0101" })));
        assert!(m.contains(&json!({ "kind": "email", "value_type": "other", "value": "jane@acme.example" })));
        assert!(m.contains(&json!({ "kind": "web", "value_type": "work", "value": "https://acme.example" })));
        assert!(m.contains(&json!({ "kind": "im", "value_type": "telegram", "value": "janedoe" })));
    }

    #[test]
    fn version_2_1_quoted_printable_and_folding() {
        let cards = parse(
            "BEGIN:VCARD\nVERSION:2.1\nN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:M=C3=BCller;J=C3=B6=\nrg\nTEL;CELL;PREF:+49 30 1234\n\
             EMAIL;HOME:jorg@\n example.de\nBDAY:19750102\nEND:VCARD\nBEGIN:VCARD\nVERSION:4.0\nFN:Solo Name\nEND:VCARD\n\
             BEGIN:VCARD\nVERSION:3.0\nNOTE:nothing to go on\nEND:VCARD\n",
        );
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].body["last_name"], "Müller");
        assert_eq!(cards[0].body["first_name"], "Jörg");
        assert_eq!(cards[0].body["birthdate"], "1975-01-02");
        let m = cards[0].body["multifields"].as_array().unwrap();
        assert!(m.contains(&json!({ "kind": "phone", "value_type": "mobile", "value": "+49 30 1234" })));
        assert!(m.contains(&json!({ "kind": "email", "value_type": "home", "value": "jorg@example.de" })));
        assert_eq!(cards[1].body["first_name"], "Solo");
        assert_eq!(cards[1].body["last_name"], "Name");
        assert_eq!(cards[1].line, 10);
    }

    #[test]
    fn encodings() {
        assert_eq!(decode("Zoë".as_bytes()), "Zoë");
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend("Zoë".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode(&utf16), "Zoë");
        assert_eq!(decode(&[b'Z', b'o', 0xEB]), "Zoë"); // Windows-1252
    }
}
