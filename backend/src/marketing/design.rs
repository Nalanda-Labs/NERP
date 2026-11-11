//! Designed emails: a campaign's message as blocks (heading, text, button, image, divider,
//! spacer, two columns, social links) with a page style, turned into email-safe HTML (tables
//! and inline styles, which mail programs understand) and a plain-text version.
//!
//! Every text is filled with the recipient's placeholders first and escaped after, so a
//! client's data can't add markup. Links (buttons, images, links in text) go through the
//! campaign's click tracking. Uploaded images are served from a signed public address,
//! since mail programs load them without signing in.

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

const KINDS: &[&str] = &["heading", "text", "button", "image", "divider", "spacer", "columns", "social"];
const NETWORKS: &[&str] = &["facebook", "instagram", "linkedin", "x", "youtube", "tiktok", "website"];
const FONTS: &[(&str, &str)] = &[
    ("sans", "Helvetica, Arial, sans-serif"),
    ("serif", "Georgia, 'Times New Roman', serif"),
    ("mono", "'Courier New', Courier, monospace"),
];

fn color(v: &Value, default: &str) -> String {
    v.as_str().filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit())).unwrap_or(default).to_lowercase()
}

fn s(v: &Value, max: usize) -> String {
    v.as_str().unwrap_or("").chars().take(max).collect()
}

fn url_ok(u: &str) -> bool {
    u.is_empty() || crate::outbound::check_url(u).is_ok() || u.starts_with("mailto:") || u.starts_with("tel:")
}

/// Checks and normalises a design: known blocks and fields only, sizes and colours bounded.
/// `own_file` says whether an uploaded image may be used (the author's own upload).
pub async fn clean(design: &Value, own_file: impl AsyncFn(Uuid) -> bool) -> AppResult<Value> {
    let blocks = design["blocks"].as_array().ok_or_else(|| AppError::bad("a design needs blocks"))?;
    if blocks.len() > 100 {
        return Err(AppError::bad("at most 100 blocks"));
    }
    let st = &design["style"];
    let font = st["font"].as_str().filter(|f| FONTS.iter().any(|x| x.0 == *f)).unwrap_or("sans");
    let style = json!({
        "background": color(&st["background"], "#f1f5f9"),
        "content": color(&st["content"], "#ffffff"),
        "text": color(&st["text"], "#1e293b"),
        "accent": color(&st["accent"], "#1a73e8"),
        "font": font,
        "preheader": s(&st["preheader"], 200),
    });
    let mut out = Vec::new();
    for b in blocks {
        let kind = b["kind"].as_str().unwrap_or("");
        if !KINDS.contains(&kind) {
            return Err(AppError::bad(format!("unknown block: {kind}")));
        }
        let align = b["align"].as_str().filter(|a| ["left", "center", "right"].contains(a)).unwrap_or("left");
        let mut m = Map::new();
        m.insert("kind".into(), json!(kind));
        match kind {
            "heading" => {
                m.insert("text".into(), json!(s(&b["text"], 300)));
                m.insert("size".into(), json!(if b["size"] == "small" { "small" } else { "large" }));
                m.insert("align".into(), json!(align));
            }
            "text" => {
                m.insert("text".into(), json!(s(&b["text"], 20_000)));
                m.insert("align".into(), json!(align));
            }
            "button" => {
                let url = s(&b["url"], 2000);
                if !url_ok(&url) {
                    return Err(AppError::bad("a button needs a web address (https://…)"));
                }
                m.insert("label".into(), json!(s(&b["label"], 100)));
                m.insert("url".into(), json!(url));
                m.insert("color".into(), json!(color(&b["color"], &style["accent"].as_str().unwrap().to_string())));
                m.insert("align".into(), json!(if b["align"].is_null() { "center" } else { align }));
            }
            "image" => {
                let url = s(&b["url"], 2000);
                let link = s(&b["link"], 2000);
                if !url_ok(&url) || !url_ok(&link) {
                    return Err(AppError::bad("an image needs a web address (https://…)"));
                }
                let file = b["file_id"].as_str().and_then(|f| Uuid::parse_str(f).ok());
                if let Some(f) = file
                    && !own_file(f).await
                {
                    return Err(AppError::bad("use an image you uploaded"));
                }
                if file.is_none() && url.is_empty() {
                    return Err(AppError::bad("choose an image"));
                }
                m.insert("file_id".into(), json!(file));
                m.insert("url".into(), json!(url));
                m.insert("alt".into(), json!(s(&b["alt"], 200)));
                m.insert("link".into(), json!(link));
                m.insert("width".into(), json!(b["width"].as_u64().unwrap_or(100).clamp(10, 100)));
                m.insert("align".into(), json!(if b["align"].is_null() { "center" } else { align }));
            }
            "divider" => {}
            "spacer" => {
                m.insert("height".into(), json!(b["height"].as_u64().unwrap_or(24).clamp(4, 120)));
            }
            "columns" => {
                m.insert("left".into(), json!(s(&b["left"], 10_000)));
                m.insert("right".into(), json!(s(&b["right"], 10_000)));
            }
            "social" => {
                let mut links = Vec::new();
                for l in b["links"].as_array().into_iter().flatten().take(10) {
                    let network = l["network"].as_str().filter(|n| NETWORKS.contains(n)).ok_or_else(|| AppError::bad("unknown social network"))?;
                    let url = s(&l["url"], 2000);
                    if url.is_empty() || !url_ok(&url) {
                        return Err(AppError::bad("social links need web addresses"));
                    }
                    links.push(json!({ "network": network, "url": url }));
                }
                m.insert("links".into(), json!(links));
                m.insert("align".into(), json!(if b["align"].is_null() { "center" } else { align }));
            }
            _ => unreachable!(),
        }
        out.push(Value::Object(m));
    }
    Ok(json!({ "style": style, "blocks": out }))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Text of a text block as HTML: paragraphs, line breaks, **bold**, *italic*, and links
/// (addresses as they are, or [label](https://…)), all through `link`.
fn rich(text: &str, accent: &str, link: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    for (i, para) in text.split("\n\n").filter(|p| !p.trim().is_empty()).enumerate() {
        if i > 0 {
            out.push_str("<br><br>");
        }
        out.push_str(&inline(para.trim_matches('\n'), accent, link).replace('\n', "<br>"));
    }
    out
}

fn inline(t: &str, accent: &str, link: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    let mut bold = false;
    let mut italic = false;
    let a = |href: &str, label: &str| format!("<a href=\"{}\" style=\"color:{accent};text-decoration:underline\">{}</a>", esc(&link(href)), esc(label));
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        // [label](https://…)
        if chars[i] == '['
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close + 2..].find(')')
        {
            let label = &rest[1..close];
            let href = &rest[close + 2..close + 2 + end];
            if (href.starts_with("http://") || href.starts_with("https://")) && !label.is_empty() {
                out.push_str(&a(href, label));
                i += rest[..close + 2 + end + 1].chars().count();
                continue;
            }
        }
        if rest.starts_with("https://") || rest.starts_with("http://") {
            let url: String = rest.chars().take_while(|c| !c.is_whitespace() && !matches!(c, '<' | '>' | '"' | ')')).collect();
            let url = url.trim_end_matches(['.', ',', ';', ':', '!', '?']).to_string();
            out.push_str(&a(&url, &url));
            i += url.chars().count();
            continue;
        }
        if rest.starts_with("**") {
            out.push_str(if bold { "</strong>" } else { "<strong>" });
            bold = !bold;
            i += 2;
            continue;
        }
        if chars[i] == '*' && (italic || rest[1..].contains('*')) {
            out.push_str(if italic { "</em>" } else { "<em>" });
            italic = !italic;
            i += 1;
            continue;
        }
        out.push_str(&esc(&chars[i].to_string()));
        i += 1;
    }
    if italic {
        out.push_str("</em>");
    }
    if bold {
        out.push_str("</strong>");
    }
    out
}

/// Plain text of a text block: markers dropped, [label](url) as "label (url)".
fn plain(t: &str, link: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(open) = rest.find('[') {
        let after = &rest[open..];
        match (after.find("]("), after.find(')')) {
            (Some(c), Some(e)) if c < e => {
                out.push_str(&rest[..open]);
                out.push_str(&format!("{} ({})", &after[1..c], link(&after[c + 2..e])));
                rest = &after[e + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "")
}

/// The public address of an uploaded image (signed, so only images used in designs are served).
pub fn image_url(state: &AppState, file: Uuid) -> String {
    let sig = crate::outbound::sign(&state.config.jwt_secret, format!("mimg:{file}").as_bytes());
    format!("{}/api/public/m/i/{file}/{}", state.config.public_url, &sig.trim_start_matches("sha256=")[..32])
}

/// Serves an image of a designed email.
pub async fn image(State(state): State<AppState>, Path((file, sig)): Path<(Uuid, String)>) -> AppResult<Response> {
    let want = image_url(&state, file);
    if !want.ends_with(&format!("/{sig}")) {
        return Err(AppError::NotFound("image"));
    }
    let f = crate::routes::files::fetch(&state, file).await.map_err(|_| AppError::NotFound("image"))?;
    let mime = f.mime_type.clone();
    if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&mime.as_str()) {
        return Err(AppError::NotFound("image"));
    }
    let bytes = crate::routes::files::read_content(&state, &f).await.map_err(|_| AppError::NotFound("image"))?;
    let mut res = bytes.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap());
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    Ok(res)
}

/// The email: HTML and plain text. `fill` puts in the recipient's placeholders; `link` makes a
/// tracked link; `footer` is the unsubscribe line (HTML and text).
pub fn render(state: &AppState, design: &Value, fill: &dyn Fn(&str) -> String, link: &dyn Fn(&str) -> String, footer: (&str, &str)) -> (String, String) {
    let st = &design["style"];
    let (bg, content, text_c, accent) = (color(&st["background"], "#f1f5f9"), color(&st["content"], "#ffffff"), color(&st["text"], "#1e293b"), color(&st["accent"], "#1a73e8"));
    let font = FONTS.iter().find(|f| Some(f.0) == st["font"].as_str()).map(|f| f.1).unwrap_or(FONTS[0].1);
    let mut rows = String::new();
    let mut txt = String::new();
    let td = |inner: &str, extra: &str| format!("<tr><td style=\"padding:8px 32px;{extra}\">{inner}</td></tr>");
    for b in design["blocks"].as_array().into_iter().flatten() {
        let align = b["align"].as_str().unwrap_or("left");
        match b["kind"].as_str().unwrap_or("") {
            "heading" => {
                let t = fill(b["text"].as_str().unwrap_or(""));
                let size = if b["size"] == "small" { 20 } else { 28 };
                rows.push_str(&td(&format!("<h{lvl} style=\"margin:8px 0;font-size:{size}px;line-height:1.25;color:{text_c};text-align:{align};font-family:{font}\">{}</h{lvl}>", esc(&t), lvl = if size == 28 { 1 } else { 2 }), ""));
                txt.push_str(&format!("{t}\n\n"));
            }
            "text" => {
                let t = fill(b["text"].as_str().unwrap_or(""));
                rows.push_str(&td(&format!("<div style=\"font-size:15px;line-height:1.6;color:{text_c};text-align:{align};font-family:{font}\">{}</div>", rich(&t, &accent, link)), ""));
                txt.push_str(&format!("{}\n\n", plain(&t, link)));
            }
            "button" => {
                let label = fill(b["label"].as_str().unwrap_or(""));
                let url = fill(b["url"].as_str().unwrap_or(""));
                let c = color(&b["color"], &accent);
                let href = link(&url);
                rows.push_str(&td(
                    &format!(
                        "<table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" align=\"{align}\"><tr><td style=\"border-radius:8px;background:{c}\"><a href=\"{}\" style=\"display:inline-block;padding:12px 24px;font-size:15px;font-weight:bold;color:#ffffff;text-decoration:none;font-family:{font};border-radius:8px\">{}</a></td></tr></table>",
                        esc(&href),
                        esc(&label)
                    ),
                    "padding-top:16px;padding-bottom:16px",
                ));
                txt.push_str(&format!("{label}: {href}\n\n"));
            }
            "image" => {
                let src = match b["file_id"].as_str().and_then(|f| Uuid::parse_str(f).ok()) {
                    Some(f) => image_url(state, f),
                    None => b["url"].as_str().unwrap_or("").to_string(),
                };
                let alt = fill(b["alt"].as_str().unwrap_or(""));
                let width = b["width"].as_u64().unwrap_or(100);
                let img = format!("<img src=\"{}\" alt=\"{}\" width=\"{}\" style=\"display:inline-block;width:{width}%;max-width:100%;height:auto;border:0\">", esc(&src), esc(&alt), 576 * width / 100);
                let target = b["link"].as_str().filter(|l| !l.is_empty()).map(|l| link(&fill(l)));
                let inner = match &target {
                    Some(h) => format!("<a href=\"{}\">{img}</a>", esc(h)),
                    None => img,
                };
                rows.push_str(&td(&format!("<div style=\"text-align:{align}\">{inner}</div>"), ""));
                if !alt.is_empty() {
                    txt.push_str(&format!("[{alt}]{}\n\n", target.map(|t| format!(" {t}")).unwrap_or_default()));
                }
            }
            "divider" => {
                rows.push_str(&td("<div style=\"border-top:1px solid #e2e8f0;height:1px;line-height:1px;font-size:1px\">&nbsp;</div>", "padding-top:16px;padding-bottom:16px"));
                txt.push_str("----\n\n");
            }
            "spacer" => {
                let h = b["height"].as_u64().unwrap_or(24);
                rows.push_str(&format!("<tr><td style=\"height:{h}px;line-height:{h}px;font-size:1px\">&nbsp;</td></tr>"));
            }
            "columns" => {
                let (l, r) = (fill(b["left"].as_str().unwrap_or("")), fill(b["right"].as_str().unwrap_or("")));
                let col = |t: &str| format!("<td class=\"col\" width=\"50%\" valign=\"top\" style=\"padding:0 8px;font-size:15px;line-height:1.6;color:{text_c};font-family:{font}\">{}</td>", rich(t, &accent, link));
                rows.push_str(&td(&format!("<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>{}{}</tr></table>", col(&l), col(&r)), ""));
                txt.push_str(&format!("{}\n\n{}\n\n", plain(&l, link), plain(&r, link)));
            }
            "social" => {
                let links: Vec<String> = b["links"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|l| {
                        let name = match l["network"].as_str().unwrap_or("") {
                            "x" => "X".to_string(),
                            "linkedin" => "LinkedIn".to_string(),
                            "youtube" => "YouTube".to_string(),
                            "tiktok" => "TikTok".to_string(),
                            n => n[..1].to_uppercase() + &n[1..],
                        };
                        (name, link(l["url"].as_str().unwrap_or("")))
                    })
                    .map(|(n, h)| {
                        txt.push_str(&format!("{n}: {h}\n"));
                        format!("<a href=\"{}\" style=\"color:{accent};text-decoration:none;font-weight:bold;margin:0 8px\">{}</a>", esc(&h), esc(&n))
                    })
                    .collect();
                txt.push('\n');
                rows.push_str(&td(&format!("<div style=\"text-align:{align};font-size:14px;font-family:{font}\">{}</div>", links.join("")), ""));
            }
            _ => {}
        }
    }
    let pre = fill(st["preheader"].as_str().unwrap_or(""));
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <style>@media (max-width:620px){{.wrap{{width:100%!important}}.col{{display:block!important;width:100%!important;padding:0!important}}}}</style></head>\
         <body style=\"margin:0;padding:0;background:{bg}\">\
         <div style=\"display:none;max-height:0;overflow:hidden;opacity:0\">{}</div>\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"background:{bg}\"><tr><td align=\"center\" style=\"padding:24px 8px\">\
         <table role=\"presentation\" class=\"wrap\" width=\"640\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"width:640px;max-width:100%;background:{content};border-radius:12px\">\
         <tr><td style=\"height:16px;font-size:1px\">&nbsp;</td></tr>{rows}<tr><td style=\"height:16px;font-size:1px\">&nbsp;</td></tr></table>\
         <p style=\"font-family:{font};font-size:12px;color:#64748b;margin:16px 0 0\">{}</p></td></tr></table></body></html>",
        esc(&pre),
        footer.0
    );
    (html, format!("{}\n--\n{}\n", txt.trim_end(), footer.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_text() {
        let link = |u: &str| format!("T({u})");
        let h = rich("Hi **Ann** <b>\n\nSee https://a.example/x. and [our offer](https://b.example) or *now*", "#123456", &link);
        assert!(h.contains("Hi <strong>Ann</strong> &lt;b&gt;<br><br>See "));
        assert!(h.contains("href=\"T(https://a.example/x)\""));
        assert!(h.contains(">https://a.example/x</a>."));
        assert!(h.contains("href=\"T(https://b.example)\" style=\"color:#123456;text-decoration:underline\">our offer</a>"));
        assert!(h.contains("<em>now</em>"));
        assert_eq!(plain("Go [here](https://c.example) **now**", &link), "Go here (T(https://c.example)) now");
    }
}
