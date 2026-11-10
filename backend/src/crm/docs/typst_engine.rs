//! Typst rendering.
//!
//! The `World` exposes exactly two virtual files: `main.typ` (the template) and
//! `data.json` (the document data), plus optional `logo.*`. Templates cannot read the
//! server's disk, reach the network or run commands, so they are safe to let CRM admins edit.
//!
//! A template can still compute for a very long time (a loop over a huge range), and a
//! running Typst compilation cannot be stopped. So templates people write are rendered in a
//! child process ([`render_isolated`]: `nerp render-typst <dir>`) that is killed at the
//! render timeout and limited in memory; NERP's own fixed templates render in-process.

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

/// Renders a template in a child process of this binary, killed after `timeout_secs`.
/// Why an isolated render failed: the template (shown to its author) or the server.
pub enum IsolatedError {
    Template(String),
    Server(String),
}

pub async fn render_isolated(template: &str, data: &serde_json::Value, logo: Option<(String, Vec<u8>)>, timeout_secs: u64) -> Result<Vec<u8>, IsolatedError> {
    use base64::Engine;
    let dir = std::env::temp_dir().join(format!("nerp-typst-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| IsolatedError::Server(e.to_string()))?;
    let server = |e: String| IsolatedError::Server(e);
    let result = async {
        let input = serde_json::json!({
            "template": template,
            "data": data,
            "files": logo.iter().map(|(n, b)| serde_json::json!([n, base64::engine::general_purpose::STANDARD.encode(b)])).collect::<Vec<_>>(),
        });
        std::fs::write(dir.join("input.json"), serde_json::to_vec(&input).map_err(|e| server(e.to_string()))?).map_err(|e| server(e.to_string()))?;
        let exe = std::env::current_exe().map_err(|e| server(e.to_string()))?;
        let dir_arg = dir.to_string_lossy().to_string();
        match super::process::run(&exe.to_string_lossy(), &["render-typst", &dir_arg], &dir, &[], timeout_secs).await {
            Ok(_) => std::fs::read(dir.join("out.pdf")).map_err(|e| server(e.to_string())),
            // A template error is printed by the child; anything else (it ran out of memory or
            // CPU time and was stopped) gets a plain explanation, not a panic message.
            Err(super::process::RunError::Failed(msg)) if msg.contains("memory allocation") || msg.contains("RUST_BACKTRACE") || msg.trim().is_empty() => {
                Err(IsolatedError::Template("the template used too much memory or time and was stopped; check it for loops".into()))
            }
            Err(super::process::RunError::Failed(msg)) => Err(IsolatedError::Template(msg.trim().to_string())),
            Err(super::process::RunError::Timeout) => Err(IsolatedError::Template(format!("the template took longer than {timeout_secs} seconds; check it for loops"))),
            Err(super::process::RunError::Spawn(e)) => Err(server(format!("could not start the renderer: {e}"))),
        }
    }
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The child process (`nerp render-typst <dir>`): renders `<dir>/input.json` to
/// `<dir>/out.pdf`. Returns the exit code.
pub fn child_main(dir: &std::path::Path) -> i32 {
    use base64::Engine;
    // A runaway template is also stopped here: memory and CPU time of this process.
    // SAFETY: setrlimit on our own process with valid structures.
    unsafe {
        let mem = libc::rlimit { rlim_cur: 4 << 30, rlim_max: 4 << 30 };
        libc::setrlimit(libc::RLIMIT_AS, &mem);
        let cpu = libc::rlimit { rlim_cur: 600, rlim_max: 600 };
        libc::setrlimit(libc::RLIMIT_CPU, &cpu);
    }
    let run = || -> Result<(), String> {
        let input: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("input.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let files = input["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| Some((f[0].as_str()?.to_string(), base64::engine::general_purpose::STANDARD.decode(f[1].as_str()?).ok()?)))
            .collect();
        let pdf = render_with(input["template"].as_str().unwrap_or_default(), &input["data"], files)?;
        std::fs::write(dir.join("out.pdf"), pdf).map_err(|e| e.to_string())
    };
    match run() {
        Ok(()) => 0,
        Err(e) => {
            eprint!("{e}");
            2
        }
    }
}

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
#[cfg_attr(not(test), allow(dead_code))]
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
