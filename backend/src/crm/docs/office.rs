//! Word/Writer templates: fill `{{placeholders}}` in a .docx, then convert to PDF with
//! headless LibreOffice.
//!
//! Template syntax (in the document text):
//!   {{client.name}}                     a value (money prints its formatted text)
//!   {{#products}} … {{/products}}       inside a table row: the row(s) repeat per product;
//!                                       inside them {{name}}, {{total}} … refer to the row
//!   {{#totals.has_tax}} … {{/totals.has_tax}}  around table rows or a whole paragraph:
//!                                       kept only when true
//!   {{^products}} … {{/products}}       the opposite: kept only when empty or false
//!
//! Word often splits text into several "runs" (e.g. after spell-checking), which would
//! break a placeholder into pieces. Paragraphs containing `{{` are therefore collapsed into
//! their first run before matching (formatting of the first run is kept).

use serde_json::Value;
use std::io::{Cursor, Read, Write};

use super::process::{run, RunError};

fn find_tag(xml: &str, from: usize, tag: &str) -> Option<usize> {
    // Matches `<w:p>` or `<w:p ` but not `<w:pPr>`.
    let mut i = from;
    while let Some(p) = xml[i..].find(tag) {
        let at = i + p;
        let next = xml.as_bytes().get(at + tag.len()).copied();
        if next == Some(b'>') || next == Some(b' ') {
            return Some(at);
        }
        i = at + tag.len();
    }
    None
}

/// The text inside every `<w:t>` of an XML fragment.
fn texts(fragment: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(start) = find_tag(fragment, i, "<w:t") {
        let Some(open_end) = fragment[start..].find('>').map(|p| start + p + 1) else { break };
        if fragment[..open_end].ends_with("/>") {
            i = open_end;
            continue;
        }
        let Some(close) = fragment[open_end..].find("</w:t>").map(|p| open_end + p) else { break };
        out.push((open_end, close));
        i = close + 6;
    }
    out
}

fn text_of(fragment: &str) -> String {
    texts(fragment).iter().map(|(a, b)| &fragment[*a..*b]).collect()
}

/// Puts a paragraph's whole text into its first `<w:t>` when it contains a placeholder.
fn merge_runs(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut i = 0;
    while let Some(start) = find_tag(xml, i, "<w:p") {
        let Some(end) = xml[start..].find("</w:p>").map(|p| start + p + 6) else { break };
        out.push_str(&xml[i..start]);
        let para = &xml[start..end];
        let spans = texts(para);
        let joined: String = spans.iter().map(|(a, b)| &para[*a..*b]).collect();
        if joined.contains("{{") && spans.len() > 1 {
            let mut rebuilt = String::new();
            let mut last = 0;
            for (n, (a, b)) in spans.iter().enumerate() {
                rebuilt.push_str(&para[last..*a]);
                if n == 0 {
                    rebuilt.push_str(&joined);
                }
                last = *b;
            }
            rebuilt.push_str(&para[last..]);
            // Make sure leading/trailing spaces survive in the first run.
            out.push_str(&rebuilt.replacen("<w:t>", "<w:t xml:space=\"preserve\">", 1));
        } else {
            out.push_str(para);
        }
        i = end;
    }
    out.push_str(&xml[i..]);
    out
}

fn lookup<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    let mut v = data;
    for part in path.split('.') {
        v = match v {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?.checked_sub(1)?)?,
            _ => return None,
        };
    }
    Some(v)
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(m) => m.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
        Value::Null | Value::Bool(false) => String::new(),
        Value::Bool(true) => "✓".into(),
        other => other.to_string(),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => m.get("text").map_or(!m.is_empty(), |t| t.as_str() != Some("")),
        _ => true,
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Replaces `{{path}}`, looking in `scope` first (a table row's item) and then `data`.
fn substitute(xml: &str, data: &Value, scope: Option<&Value>) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut i = 0;
    while let Some(p) = xml[i..].find("{{") {
        let start = i + p;
        let Some(end) = xml[start..].find("}}").map(|q| start + q) else { break };
        out.push_str(&xml[i..start]);
        let key = xml[start + 2..end].trim();
        if !(key.starts_with('#') || key.starts_with('^') || key.starts_with('/')) {
            let v = scope.and_then(|s| lookup(s, key)).or_else(|| lookup(data, key));
            let text = v.map(display).unwrap_or_default();
            out.push_str(&xml_escape(&text).replace('\n', "</w:t><w:br/><w:t xml:space=\"preserve\">"));
        }
        i = end + 2;
    }
    out.push_str(&xml[i..]);
    out
}

/// Expands `{{#name}}…{{/name}}` blocks that span whole table rows.
fn expand_rows(xml: &str, data: &Value) -> String {
    // Split into a sequence of (is_row, fragment).
    let mut parts: Vec<(bool, &str)> = Vec::new();
    let mut i = 0;
    while let Some(start) = find_tag(xml, i, "<w:tr") {
        let Some(end) = xml[start..].find("</w:tr>").map(|p| start + p + 7) else { break };
        parts.push((false, &xml[i..start]));
        parts.push((true, &xml[start..end]));
        i = end;
    }
    parts.push((false, &xml[i..]));

    let mut out = String::with_capacity(xml.len());
    let mut k = 0;
    while k < parts.len() {
        let (is_row, frag) = parts[k];
        let text = if is_row { text_of(frag) } else { String::new() };
        let Some((name, inverted)) = row_section(&text) else {
            out.push_str(frag);
            k += 1;
            continue;
        };
        // Collect rows up to the one holding {{/name}}.
        let close = format!("{{{{/{name}}}}}");
        let mut block = String::new();
        let mut j = k;
        loop {
            block.push_str(parts[j].1);
            if parts[j].0 && text_of(parts[j].1).contains(&close) || j + 1 >= parts.len() {
                break;
            }
            j += 1;
        }
        let open = format!("{{{{{}{name}}}}}", if inverted { '^' } else { '#' });
        let block = block.replace(&open, "").replace(&close, "");
        match lookup(data, &name) {
            v if inverted => {
                if !truthy(v) {
                    out.push_str(&block);
                }
            }
            Some(Value::Array(items)) => {
                for item in items {
                    out.push_str(&substitute(&block, data, Some(item)));
                }
            }
            v if truthy(v) => out.push_str(&block),
            _ => {}
        }
        k = j + 1;
    }
    out
}

/// The section a table row opens: the first `{{#name}}` or `{{^name}}` in it. A row whose
/// own text closes that section before its end only holds a paragraph-level conditional
/// (an optional line in a cell), so it is not a row section.
fn row_section(text: &str) -> Option<(String, bool)> {
    let hash = text.find("{{#");
    let caret = text.find("{{^");
    let (at, inverted) = match (hash, caret) {
        (Some(h), Some(c)) => if c < h { (c, true) } else { (h, false) },
        (Some(h), None) => (h, false),
        (None, Some(c)) => (c, true),
        (None, None) => return None,
    };
    let rest = &text[at + 3..];
    let name = rest[..rest.find("}}")?].trim().to_string();
    let close = format!("{{{{/{name}}}}}");
    if text.contains(&close) && !text.trim_end().ends_with(&close) {
        return None;
    }
    Some((name, inverted))
}

/// Drops paragraphs wrapped in `{{#name}}…{{/name}}` when `name` is empty or false, so
/// optional lines (e.g. a tax number) leave no blank line.
fn conditional_paragraphs(xml: &str, data: &Value) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut i = 0;
    while let Some(start) = find_tag(xml, i, "<w:p") {
        let Some(end) = xml[start..].find("</w:p>").map(|p| start + p + 6) else { break };
        out.push_str(&xml[i..start]);
        let para = &xml[start..end];
        let text = text_of(para);
        let (inverted, rest) = match (text.strip_prefix("{{#"), text.strip_prefix("{{^")) {
            (Some(r), _) => (false, Some(r)),
            (_, Some(r)) => (true, Some(r)),
            _ => (false, None),
        };
        let name = rest
            .and_then(|t| t.find("}}").map(|e| t[..e].trim().to_string()))
            .filter(|n| text.trim_end().ends_with(&format!("{{{{/{n}}}}}")));
        let keep = match name {
            Some(n) if inverted => !truthy(lookup(data, &n)),
            Some(n) => matches!(lookup(data, &n), Some(Value::Array(_))) || truthy(lookup(data, &n)),
            None => true,
        };
        if keep {
            out.push_str(para);
        }
        i = end;
    }
    out.push_str(&xml[i..]);
    out
}

pub fn fill_xml(xml: &str, data: &Value) -> String {
    let rows = expand_rows(&merge_runs(xml), data);
    substitute(&conditional_paragraphs(&rows, data), data, None)
}

fn is_text_part(name: &str) -> bool {
    name == "word/document.xml" || (name.starts_with("word/header") || name.starts_with("word/footer")) && name.ends_with(".xml")
}

/// Fills a .docx template with data.
pub fn fill_docx(template: &[u8], data: &Value) -> Result<Vec<u8>, String> {
    let mut zin = zip::ZipArchive::new(Cursor::new(template)).map_err(|_| "the template is not a valid .docx file".to_string())?;
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zout = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).map_err(|e| e.to_string())?;
            let name = f.name().map_err(|e| e.to_string())?.to_string();
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if is_text_part(&name) {
                let xml = String::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8"))?;
                bytes = fill_xml(&xml, data).into_bytes();
            }
            zout.start_file(name, opts).map_err(|e| e.to_string())?;
            zout.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        zout.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf.into_inner())
}

/// Converts a document with headless LibreOffice. `ext` is the input's extension and
/// `to` the target format (e.g. "pdf", "docx").
pub async fn convert(bin: &str, input: &[u8], ext: &str, to: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
    let dir = std::env::temp_dir().join(format!("nerp-lo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let result = async {
        let src = dir.join(format!("doc.{ext}"));
        std::fs::write(&src, input).map_err(|e| e.to_string())?;
        // A private profile per conversion lets several run in parallel.
        let profile = format!("-env:UserInstallation=file://{}", dir.join("profile").display());
        let out = dir.join("out");
        let args = ["--headless", "--norestore", "--nolockcheck", "--nodefault", &profile, "--convert-to", to, "--outdir", out.to_str().unwrap(), src.to_str().unwrap()];
        match run(bin, &args, &dir, &[], timeout_secs).await {
            Ok(_) => {}
            Err(RunError::Timeout) => return Err("LibreOffice took too long and was stopped".into()),
            Err(RunError::Spawn(e)) => return Err(format!("could not start LibreOffice: {e}")),
            Err(RunError::Failed(log)) => return Err(format!("LibreOffice failed: {}", log.trim())),
        }
        std::fs::read(out.join(format!("doc.{to}"))).map_err(|_| "LibreOffice produced no output".to_string())
    }
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data() -> Value {
        json!({
            "client": { "name": "Ana <&> \"Co\"" },
            "totals": { "total": { "text": "€10.00", "value": "10" }, "has_tax": false },
            "products": [{ "name": "A", "total": { "text": "€4" } }, { "name": "B", "total": { "text": "€6" } }],
        })
    }

    #[test]
    fn placeholder_split_across_runs_is_filled_and_escaped() {
        let xml = r#"<w:p><w:r><w:t>Dear {{cli</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>ent.name}}, total {{totals.total}}</w:t></w:r></w:p>"#;
        let out = fill_xml(xml, &data());
        assert!(out.contains("Dear Ana &lt;&amp;&gt; &quot;Co&quot;, total €10.00"), "{out}");
        assert!(!out.contains("{{"));
    }

    #[test]
    fn rows_repeat_per_product_and_conditional_rows_drop() {
        let xml = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>{{#products}}{{name}}</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>{{total}}{{/products}}</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>{{#totals.has_tax}}Tax{{/totals.has_tax}}</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let out = fill_xml(xml, &data());
        assert_eq!(out.matches("<w:tr>").count(), 3, "{out}");
        assert!(out.contains(">A<") && out.contains(">€4<") && out.contains(">B<") && out.contains(">€6<"));
        assert!(!out.contains("Tax"));
    }

    #[test]
    fn conditional_paragraph_dropped_when_empty() {
        let xml = r#"<w:p><w:r><w:t>{{#client.tax}}VAT {{client.tax}}{{/client.tax}}</w:t></w:r></w:p><w:p><w:r><w:t>{{#totals.total}}T {{totals.total}}{{/totals.total}}</w:t></w:r></w:p>"#;
        let out = fill_xml(xml, &data());
        assert!(!out.contains("VAT"), "{out}");
        assert!(out.contains("T €10.00") && out.matches("<w:p>").count() == 1, "{out}");
    }

    #[test]
    fn inverted_sections_show_only_when_empty() {
        let xml = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>{{^products}}No items{{/products}}</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>{{^totals.has_tax}}Tax free</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>0{{/totals.has_tax}}</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>{{^client.name}}Anonymous{{/client.name}}</w:t></w:r></w:p>"#;
        let out = fill_xml(xml, &data());
        assert!(!out.contains("No items") && !out.contains("Anonymous"), "{out}");
        assert!(out.contains(">Tax free<") && out.contains(">0<") && !out.contains("{{"), "{out}");
        let mut empty = data();
        empty["products"] = json!([]);
        assert!(fill_xml(xml, &empty).contains(">No items<"));
    }

    #[test]
    fn optional_line_in_a_cell_does_not_make_a_row_section() {
        // A cell with an optional paragraph followed by other content keeps its row.
        let xml = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>{{#client.tax}}VAT {{client.tax}}{{/client.tax}}</w:t></w:r></w:p><w:p><w:r><w:t>{{client.name}}</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Contact</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let out = fill_xml(xml, &data());
        assert!(out.contains("<w:tr>") && out.contains("Contact") && out.contains("Ana &lt;") && !out.contains("VAT"), "{out}");
    }

    #[test]
    fn pprops_are_not_mistaken_for_paragraphs() {
        let xml = r#"<w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>{{client.name}}</w:t></w:r></w:p>"#;
        assert!(fill_xml(xml, &data()).contains("<w:pPr><w:jc w:val=\"right\"/></w:pPr>"));
    }
}
