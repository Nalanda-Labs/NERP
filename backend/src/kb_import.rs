//! Knowledge base import: a Markdown or HTML file, or a ZIP of them — a folder tree of
//! Markdown files (as Notion, Obsidian and other tools export them) or a Confluence HTML
//! export — becomes pages of a space, keeping the tree, the images and the links between
//! pages.
//!
//! * Folders become parent pages; a page file next to a folder of the same name (Notion's
//!   `Page abc123.md` + `Page abc123/`) is that parent. Notion's 32-character ids are taken
//!   out of titles.
//! * A Confluence export's `index.html` gives the tree; its pages' breadcrumbs, headers and
//!   footers are left out (the `main-content` part is kept).
//! * HTML becomes Markdown. Images (PNG, JPEG, GIF, WebP) are stored as the page's files;
//!   links to other imported pages point to them.

use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/kb/spaces/{id}/import", post(import))
}

const MAX_ENTRIES: usize = 3000;
const MAX_TOTAL: u64 = 300 * 1024 * 1024;
const MAX_PAGE: usize = 500_000;

#[derive(Deserialize)]
struct ImportBody {
    file_id: Uuid,
    /// Put the imported pages under this page (default: top level).
    parent_id: Option<Uuid>,
}

/// A page to make, before it has an id.
#[derive(Debug, Default, Clone)]
struct Draft {
    /// Its path in the archive (or a folder's path for folder pages).
    path: String,
    title: String,
    body: String,
    parent: Option<String>,
}

fn ext(p: &str) -> String {
    p.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

fn is_page(p: &str) -> bool {
    ["md", "markdown", "html", "htm", "txt"].contains(&ext(p).as_str())
}

fn image_mime(p: &str) -> Option<&'static str> {
    Some(match ext(p).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// A file or folder name as a title: no extension, no Notion id, no underscores.
pub fn title_of(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let base = match base.rsplit_once('.') {
        Some((b, e)) if is_page(&format!("x.{e}")) => b,
        _ => base,
    };
    // "Title 0123456789abcdef0123456789abcdef" (Notion) or "Title_123456" (Confluence).
    let base = match base.rsplit_once(' ') {
        Some((t, id)) if id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()) => t,
        _ => base,
    };
    let base = match base.rsplit_once('_') {
        Some((t, id)) if !t.is_empty() && id.len() >= 5 && id.chars().all(|c| c.is_ascii_digit()) => t,
        _ => base,
    };
    let t = base.replace(['_', '+'], " ");
    let t = t.trim();
    if t.is_empty() { "Untitled".into() } else { t.chars().take(200).collect() }
}

fn parent_dir(p: &str) -> Option<String> {
    p.trim_end_matches('/').rsplit_once('/').map(|(d, _)| d.to_string())
}

/// The part of a page worth keeping, as Markdown, and its title if the page names one.
pub fn html_to_md(html: &str) -> (Option<String>, String) {
    let lower = html.to_ascii_lowercase();
    let title = find_between(html, &lower, "<title>", "</title>")
        .map(|t| decode_entities(t.trim()))
        .map(|t| t.split(" : ").last().unwrap_or(&t).trim().to_string())
        .filter(|t| !t.is_empty());
    // Confluence keeps the page in #main-content; others in <main>, <article> or <body>.
    let part = section(html, &lower, "id=\"main-content\"").or_else(|| element(html, &lower, "main")).or_else(|| element(html, &lower, "article")).or_else(|| element(html, &lower, "body")).unwrap_or(html);
    let md = htmd::HtmlToMarkdown::builder().skip_tags(vec!["script", "style", "head", "nav", "footer", "header"]).build().convert(part).unwrap_or_default();
    (title, md.trim().to_string())
}

fn find_between<'a>(s: &'a str, lower: &str, open: &str, close: &str) -> Option<&'a str> {
    let a = lower.find(open)? + open.len();
    let b = lower[a..].find(close)? + a;
    Some(&s[a..b])
}

/// The element that carries `attr` (from its tag to the matching close of that tag name).
fn section<'a>(s: &'a str, lower: &str, attr: &str) -> Option<&'a str> {
    let at = lower.find(attr)?;
    let start = lower[..at].rfind('<')?;
    let name: String = lower[start + 1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    let open_end = lower[at..].find('>')? + at + 1;
    matching_close(s, lower, &name, open_end)
}

fn element<'a>(s: &'a str, lower: &str, name: &str) -> Option<&'a str> {
    let start = lower.find(&format!("<{name}"))?;
    let open_end = lower[start..].find('>')? + start + 1;
    matching_close(s, lower, name, open_end)
}

fn matching_close<'a>(s: &'a str, lower: &str, name: &str, from: usize) -> Option<&'a str> {
    let (open, close) = (format!("<{name}"), format!("</{name}"));
    let mut depth = 1;
    let mut i = from;
    while depth > 0 {
        let next_open = lower[i..].find(&open).map(|x| x + i);
        let next_close = lower[i..].find(&close).map(|x| x + i)?;
        match next_open {
            Some(o) if o < next_close => {
                depth += 1;
                i = o + open.len();
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[from..next_close]);
                }
                i = next_close + close.len();
            }
        }
    }
    None
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ")
}

/// A Confluence `index.html`: each linked page and the page it sits under (nested lists).
pub fn confluence_tree(index: &str) -> Vec<(String, Option<String>)> {
    let lower = index.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut stack: Vec<Option<String>> = vec![None];
    let mut last: Option<String> = None;
    let mut i = 0;
    while i < lower.len() {
        let rest = &lower[i..];
        if rest.starts_with("<ul") {
            stack.push(last.clone());
            i += 3;
        } else if rest.starts_with("</ul") {
            stack.pop();
            if stack.is_empty() {
                stack.push(None);
            }
            i += 4;
        } else if rest.starts_with("<a ") {
            if let Some(h) = rest.find("href=\"") {
                let a = i + h + 6;
                if let Some(e) = lower[a..].find('"') {
                    let href = index[a..a + e].to_string();
                    if href.ends_with(".html") && !href.contains("://") && !href.starts_with('#') && !href.contains("attachments/") {
                        out.push((href.clone(), stack.last().cloned().flatten()));
                        last = Some(href);
                    }
                }
            }
            i += 3;
        } else {
            i += rest.chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    out
}

/// Relative path `target` seen from the file at `from`, normalised (`..` and `.` resolved).
fn resolve(from: &str, target: &str) -> String {
    let target = target.split(['#', '?']).next().unwrap_or("");
    let target = urlish_decode(target);
    let dir = parent_dir(from).filter(|_| !target.starts_with('/')).unwrap_or_default();
    let mut parts: Vec<String> = dir.split('/').filter(|x| !x.is_empty()).map(String::from).collect();
    for seg in target.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s.to_string()),
        }
    }
    parts.join("/")
}

fn urlish_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every Markdown link or image target in a body: (start, end) of the target text.
fn link_targets(md: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let b = md.as_bytes();
    let mut i = 0;
    while let Some(p) = md[i..].find("](") {
        let start = i + p + 2;
        let mut depth = 1;
        let mut j = start;
        while j < b.len() && depth > 0 {
            match b[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'\n' => break,
                _ => {}
            }
            if depth > 0 {
                j += 1;
            }
        }
        if depth == 0 {
            // `<…>` targets and a title after a space.
            let raw = &md[start..j];
            let (s, e) = if raw.starts_with('<') {
                (start + 1, start + raw.find('>').unwrap_or(raw.len()))
            } else {
                (start, start + raw.find(" \"").unwrap_or(raw.len()))
            };
            out.push((s, e));
        }
        i = j.max(start);
        if i >= md.len() {
            break;
        }
    }
    out
}

struct Archive {
    pages: BTreeMap<String, String>,
    assets: HashMap<String, Vec<u8>>,
}

fn read_zip(bytes: &[u8]) -> AppResult<Archive> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| AppError::bad("that isn't a ZIP file"))?;
    if z.len() > MAX_ENTRIES {
        return Err(AppError::bad(format!("the archive holds more than {MAX_ENTRIES} files")));
    }
    let mut total = 0u64;
    let mut pages = BTreeMap::new();
    let mut assets = HashMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|_| AppError::bad("the archive is damaged"))?;
        if f.is_dir() {
            continue;
        }
        let Some(name) = f.enclosed_name().map(|p| p.to_string_lossy().replace('\\', "/")) else { continue };
        if name.starts_with("__MACOSX/") || name.rsplit('/').next().is_some_and(|n| n.starts_with('.')) {
            continue;
        }
        let page = is_page(&name);
        if !page && image_mime(&name).is_none() {
            continue;
        }
        total += f.size();
        if total > MAX_TOTAL {
            return Err(AppError::bad("the archive is too big when unpacked (300 MB at most)"));
        }
        let mut buf = Vec::with_capacity(f.size().min(64 << 20) as usize);
        f.by_ref().take(64 << 20).read_to_end(&mut buf).map_err(|_| AppError::bad("the archive is damaged"))?;
        if page {
            pages.insert(name, crate::crm::vcard::decode(&buf));
        } else {
            assets.insert(name, buf);
        }
    }
    Ok(Archive { pages, assets })
}

/// Pages (in the order to make them, parents first) from an archive's files.
fn drafts(a: &Archive) -> Vec<Draft> {
    let mut list: Vec<Draft> = Vec::new();
    let confluence = a.pages.keys().find(|k| k.rsplit('/').next() == Some("index.html")).and_then(|k| a.pages.get(k).map(|t| (k.clone(), t.clone())));
    let tree: HashMap<String, Option<String>> = match &confluence {
        Some((ipath, index)) if index.to_ascii_lowercase().contains("confluence") || index.contains("<ul") => confluence_tree(index)
            .into_iter()
            .map(|(href, parent)| (resolve(ipath, &href), parent.map(|p| resolve(ipath, &p))))
            .collect(),
        _ => HashMap::new(),
    };
    for (path, text) in &a.pages {
        if confluence.as_ref().is_some_and(|(i, _)| i == path) && !tree.is_empty() {
            continue;
        }
        let (named, mut body) = match ext(path).as_str() {
            "html" | "htm" => html_to_md(text),
            _ => (None, text.trim_start_matches('\u{feff}').to_string()),
        };
        // A Markdown page's first heading is its title.
        let mut title = named;
        if title.is_none() && let Some(first) = body.lines().next()
            && let Some(h) = first.strip_prefix("# ")
        {
            title = Some(h.trim().to_string());
            body = body.lines().skip(1).collect::<Vec<_>>().join("\n").trim_start().to_string();
        }
        let parent = if !tree.is_empty() { tree.get(path).cloned().flatten() } else { parent_dir(path) };
        list.push(Draft { path: path.clone(), title: title.unwrap_or_else(|| title_of(path)).chars().take(200).collect(), body, parent });
    }
    if tree.is_empty() {
        // Folders: the page of the same name next to them, or a page of their own.
        let mut folders: Vec<String> = list.iter().filter_map(|d| d.parent.clone()).collect();
        let mut i = 0;
        while i < folders.len() {
            if let Some(up) = parent_dir(&folders[i]) {
                folders.push(up);
            }
            i += 1;
        }
        folders.sort();
        folders.dedup();
        // A single top folder (the archive's own name) isn't a page.
        let tops: Vec<&String> = folders.iter().filter(|f| !f.contains('/')).collect();
        let skip_top = tops.len() == 1 && !list.iter().any(|d| d.parent.is_none()) && !list.iter().any(|d| same_name(&d.path, tops[0]));
        let skip = if skip_top { Some(tops[0].clone()) } else { None };
        let mut owner: HashMap<String, String> = HashMap::new();
        for f in &folders {
            if Some(f) == skip.as_ref() {
                continue;
            }
            match list.iter().find(|d| same_name(&d.path, f)) {
                Some(d) => {
                    owner.insert(f.clone(), d.path.clone());
                }
                None => {
                    let parent = parent_dir(f).filter(|p| Some(p) != skip.as_ref());
                    list.push(Draft { path: format!("{f}/"), title: title_of(f), body: String::new(), parent });
                    owner.insert(f.clone(), format!("{f}/"));
                }
            }
        }
        for d in list.iter_mut() {
            d.parent = d.parent.as_ref().filter(|p| Some(*p) != skip.as_ref()).and_then(|p| owner.get(p).cloned());
            if d.path.ends_with('/') {
                d.parent = parent_dir(d.path.trim_end_matches('/')).filter(|p| Some(p) != skip.as_ref()).and_then(|p| owner.get(&p).cloned());
            }
        }
    }
    // Parents first.
    let mut ordered: Vec<Draft> = Vec::new();
    let mut left = list;
    for _ in 0..64 {
        if left.is_empty() {
            break;
        }
        let (ready, wait): (Vec<Draft>, Vec<Draft>) = left.into_iter().partition(|d| d.parent.as_ref().is_none_or(|p| ordered.iter().any(|o| &o.path == p)));
        if ready.is_empty() {
            // A loop or a missing parent: put the rest at the top.
            ordered.extend(wait.into_iter().map(|mut d| {
                d.parent = None;
                d
            }));
            break;
        }
        ordered.extend(ready);
        left = wait;
    }
    ordered
}

/// `A/Page.md` names the folder `A/Page` (also with Notion's id: `A/Page 1234…` both sides).
fn same_name(page: &str, folder: &str) -> bool {
    page.rsplit_once('.').map(|(b, _)| b) == Some(folder)
}

async fn import(State(state): State<AppState>, auth: AuthUser, Path(space): Path<Uuid>, Json(b): Json<ImportBody>) -> AppResult<Json<Value>> {
    crate::kb::editable(&state, &auth, space).await?;
    crate::kb::check_parent(&state, space, b.parent_id, None).await?;
    let f = crate::routes::files::fetch(&state, b.file_id).await?;
    if f.owner_id != Some(auth.id) {
        return Err(AppError::bad("upload the file first"));
    }
    let bytes = crate::routes::files::read_content(&state, &f).await?;
    let name = f.original_name;
    let archive = if ext(&name) == "zip" || bytes.starts_with(b"PK\x03\x04") {
        read_zip(&bytes)?
    } else if is_page(&name) {
        let mut pages = BTreeMap::new();
        pages.insert(name.clone(), crate::crm::vcard::decode(&bytes));
        Archive { pages, assets: HashMap::new() }
    } else {
        return Err(AppError::bad("import Markdown or HTML files, or a ZIP of them"));
    };
    let list = drafts(&archive);
    if list.is_empty() {
        return Err(AppError::bad("no Markdown or HTML pages found"));
    }
    // Make the pages (so links between them can point to their ids), then fill in bodies.
    let mut ids: HashMap<String, Uuid> = HashMap::new();
    let mut tx = state.db.begin().await?;
    let mut sort: i32 = sqlx::query_scalar!(r#"SELECT COALESCE(MAX(sort), 0) AS "v!" FROM kb_pages WHERE space_id = $1 AND parent_id IS NOT DISTINCT FROM $2"#, space, b.parent_id).fetch_one(&mut *tx).await?;
    for d in &list {
        sort += 10;
        let parent = d.parent.as_ref().and_then(|p| ids.get(p).copied()).or(b.parent_id);
        let id: Uuid = sqlx::query_scalar!("INSERT INTO kb_pages (space_id, parent_id, title, body, sort, created_by, updated_by) VALUES ($1, $2, $3, '', $4, $5, $5) RETURNING id", space, parent, &d.title, sort, auth.id)
            .fetch_one(&mut *tx)
            .await?;
        ids.insert(d.path.clone(), id);
    }
    tx.commit().await?;
    let mut stored: HashMap<String, Uuid> = HashMap::new();
    let mut images = 0;
    let mut out = Vec::new();
    for d in &list {
        let id = ids[&d.path];
        let mut body = d.body.clone();
        // Links and images, last first so positions stay right.
        for (s, e) in link_targets(&body).into_iter().rev() {
            let target = body[s..e].to_string();
            if target.contains("://") || target.starts_with('#') || target.starts_with("mailto:") || target.is_empty() {
                continue;
            }
            let full = resolve(&d.path, &target);
            if let Some(p) = ids.get(&full) {
                body.replace_range(s..e, &format!("/kb/page/{p}"));
            } else if let (Some(bytes), Some(mime)) = (archive.assets.get(&full), image_mime(&full)) {
                let fid = match stored.get(&full) {
                    Some(f) => *f,
                    None => {
                        let f = crate::routes::files::store_bytes(&state, Some(auth.id), full.rsplit('/').next().unwrap_or("image"), mime, bytes).await?;
                        stored.insert(full.clone(), f);
                        images += 1;
                        f
                    }
                };
                sqlx::query!("INSERT INTO kb_files (page_id, file_id, added_by) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING", id, fid, auth.id).execute(&state.db).await?;
                body.replace_range(s..e, &format!("/api/files/{fid}/content"));
            }
        }
        if body.len() > MAX_PAGE {
            body.truncate(body.char_indices().take_while(|(i, _)| *i < MAX_PAGE).last().map(|(i, _)| i).unwrap_or(0));
        }
        sqlx::query!("UPDATE kb_pages SET body = $2 WHERE id = $1", id, &body).execute(&state.db).await?;
        sqlx::query!("INSERT INTO kb_versions (page_id, version, title, body, saved_by) VALUES ($1, 1, $2, $3, $4) ON CONFLICT DO NOTHING", id, &d.title, &body, auth.id).execute(&state.db).await?;
        out.push(json!({ "id": id, "title": d.title, "parent_id": d.parent.as_ref().and_then(|p| ids.get(p)).copied().or(b.parent_id) }));
    }
    Ok(Json(json!({ "pages": out, "images": images })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title_of("Docs/Getting started 0123456789abcdef0123456789abcdef.md"), "Getting started");
        assert_eq!(title_of("Onboarding_98765.html"), "Onboarding");
        assert_eq!(title_of("team_handbook.md"), "team handbook");
    }

    #[test]
    fn html() {
        let (t, md) = html_to_md("<html><head><title>Space : Release notes</title></head><body><div id=\"breadcrumbs\">x</div><div id=\"main-content\"><h2>v2</h2><p>Some <b>bold</b> <a href=\"Other_123.html\">link</a></p><div>inner</div></div><div id=\"footer\">f</div></body></html>");
        assert_eq!(t.as_deref(), Some("Release notes"));
        assert!(md.contains("## v2") && md.contains("**bold**") && md.contains("[link](Other_123.html)") && md.contains("inner"), "{md}");
        // The breadcrumbs and footer around the page are left out.
        assert!(!md.contains('x') && !md.ends_with('f'), "{md}");
    }

    #[test]
    fn confluence_index() {
        let t = confluence_tree(r#"<ul><li><a href="Home_1.html">Home</a><ul><li><a href="Child_2.html">C</a><ul><li><a href="Grand_3.html">G</a></li></ul></li><li><a href="Sib_4.html">S</a></li></ul></li></ul>"#);
        assert_eq!(t, vec![("Home_1.html".into(), None), ("Child_2.html".into(), Some("Home_1.html".into())), ("Grand_3.html".into(), Some("Child_2.html".into())), ("Sib_4.html".into(), Some("Home_1.html".into()))]);
    }

    #[test]
    fn links_and_paths() {
        let md = "See [a](b%20c.md) and ![img](../img/x.png \"t\") and [web](https://x.example).";
        let ts: Vec<&str> = link_targets(md).into_iter().map(|(s, e)| &md[s..e]).collect();
        assert_eq!(ts, vec!["b%20c.md", "../img/x.png", "https://x.example"]);
        assert_eq!(resolve("A/B/page.md", "../img/x.png"), "A/img/x.png");
        assert_eq!(resolve("A/page.md", "b%20c.md#top"), "A/b c.md");
    }

    #[test]
    fn folder_trees() {
        let mut pages = BTreeMap::new();
        for p in ["Export/Team 0123456789abcdef0123456789abcdef.md", "Export/Team 0123456789abcdef0123456789abcdef/Rules.md", "Export/Other/Deep.md"] {
            pages.insert(p.to_string(), "# T\nbody".to_string());
        }
        let d = drafts(&Archive { pages, assets: HashMap::new() });
        let by = |path: &str| d.iter().find(|x| x.path == path).unwrap();
        // The archive's single top folder isn't a page; "Other" becomes one.
        assert!(by("Export/Team 0123456789abcdef0123456789abcdef.md").parent.is_none());
        assert_eq!(by("Export/Team 0123456789abcdef0123456789abcdef/Rules.md").parent.as_deref(), Some("Export/Team 0123456789abcdef0123456789abcdef.md"));
        assert_eq!(by("Export/Other/").title, "Other");
        assert_eq!(by("Export/Other/Deep.md").parent.as_deref(), Some("Export/Other/"));
        assert!(!d.iter().any(|x| x.path == "Export/"));
    }
}
