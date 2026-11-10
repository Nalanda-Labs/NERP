//! Typst rendering, in-process.
//!
//! The `World` exposes exactly two virtual files: `main.typ` (the template) and
//! `data.json` (the document data), plus optional `logo.*`. Templates cannot read the
//! server's disk, reach the network or run commands, so they are safe to let CRM admins edit.

use std::sync::OnceLock;
use typst::{
    diag::{FileError, FileResult, Severity},
    foundations::{Bytes, Datetime, Duration},
    syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot},
    text::{Font, FontBook},
    utils::LazyHash,
    Library, LibraryExt, World, WorldExt,
};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;

/// Fonts are scanned once: system fonts (fontconfig dirs) plus Typst's embedded defaults.
fn fonts() -> &'static FontStore {
    static FONTS: OnceLock<FontStore> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut store = FontStore::new();
        store.extend(typst_kit::fonts::system());
        if let Ok(dir) = std::env::var("PDF_FONT_DIR") {
            store.extend(typst_kit::fonts::scan(std::path::Path::new(&dir)));
        }
        store.extend(typst_kit::fonts::embedded());
        tracing::info!(fonts = store.book().families().count(), "loaded fonts for document rendering");
        store
    })
}

fn library() -> &'static LazyHash<Library> {
    static LIB: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIB.get_or_init(|| LazyHash::new(Library::default()))
}

fn id(path: &str) -> FileId {
    RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).expect("valid path")).intern()
}

struct DocWorld {
    main: Source,
    data: Bytes,
    /// Other files the template may read (a logo, signature images…), by name.
    files: Vec<(String, Bytes)>,
}

impl World for DocWorld {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }
    fn book(&self) -> &LazyHash<FontBook> {
        fonts().book()
    }
    fn main(&self) -> FileId {
        self.main.id()
    }
    fn source(&self, file: FileId) -> FileResult<Source> {
        if file == self.main.id() { Ok(self.main.clone()) } else { Err(FileError::AccessDenied) }
    }
    fn file(&self, file: FileId) -> FileResult<Bytes> {
        if file == id("/data.json") {
            return Ok(self.data.clone());
        }
        for (name, bytes) in &self.files {
            if file == id(&format!("/{name}")) {
                return Ok(bytes.clone());
            }
        }
        Err(FileError::AccessDenied)
    }
    fn font(&self, index: usize) -> Option<Font> {
        fonts().font(index)
    }
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        use chrono::Datelike;
        let now = chrono::Utc::now();
        Datetime::from_ymd(now.year(), now.month() as u8, now.day() as u8)
    }
}

/// Renders a Typst template with `data` (JSON). Returns the PDF, or the compiler's
/// error messages with line numbers.
pub fn render(template: &str, data: &serde_json::Value, logo: Option<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
    render_with(template, data, logo.into_iter().collect())
}

/// Like [`render`], with any number of extra files the template can read.
pub fn render_with(template: &str, data: &serde_json::Value, files: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
    let main = Source::new(id("/main.typ"), template.to_string());
    let world = DocWorld {
        main: main.clone(),
        data: Bytes::new(serde_json::to_vec(data).map_err(|e| e.to_string())?),
        files: files.into_iter().map(|(n, b)| (n, Bytes::new(b))).collect(),
    };
    let doc = typst::compile::<PagedDocument>(&world).output.map_err(|errs| {
        errs.iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| {
                let line = world.range(d.span).and_then(|r| main.lines().byte_to_line(r.start)).map(|l| format!("line {}: ", l + 1)).unwrap_or_default();
                format!("{line}{}", d.message)
            })
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if doc.pages().len() > 200 {
        return Err("documents are limited to 200 pages".into());
    }
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(|errs| errs.iter().map(|d| d.message.to_string()).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_unicode_and_reads_data() {
        let data = serde_json::json!({ "name": "नमस्ते مرحبا 你好 Привет Γειά" });
        let pdf = render("#let d = json(\"data.json\")\nHello #d.name", &data, None).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
    }

    #[test]
    fn templates_cannot_read_server_files() {
        let err = render("#read(\"/etc/passwd\")", &serde_json::json!({}), None).unwrap_err();
        assert!(err.contains("line 1"), "{err}");
    }
}
