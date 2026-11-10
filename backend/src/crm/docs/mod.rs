//! CRM documents: quotes, invoices and other PDFs from templates in Typst, LuaLaTeX or
//! LibreOffice (Word/Writer) formats.

pub mod api;
pub mod browse;
pub mod data;
pub mod latex;
pub mod office;
pub mod process;
pub mod typst_engine;

pub use api::{can_view_file, detect, engines, run_job, seed, JOB};

#[cfg(test)]
mod engine_tests {
    //! Renders the built-in templates with every engine. Needs lualatex and soffice, so
    //! run with `cargo test -- --ignored`. PDFs are left in the temp dir for inspection.
    use super::*;

    fn hostile() -> serde_json::Value {
        let mut d = data::sample("de-DE", "Invoice");
        d["client"]["name"] = serde_json::json!("Ünïcødé & Co \\input{/etc/passwd} 100% {{client.name}} <w:t> $x$ #hash");
        d["client"]["contact_person"] = serde_json::json!("अनन्या शर्मा · محمد علي · 王小明 · Дмитрий · Γιώργος");
        d
    }

    fn out(name: &str, pdf: &[u8]) {
        assert!(pdf.starts_with(b"%PDF-"), "{name} is not a PDF");
        std::fs::write(std::env::temp_dir().join(format!("nerp-test-{name}.pdf")), pdf).unwrap();
    }

    #[test]
    #[ignore]
    fn typst_builtin() {
        let pdf = typst_engine::render(include_str!("../../../assets/doc-templates/invoice.typ"), &hostile(), None).unwrap();
        out("typst", &pdf);
    }

    #[tokio::test]
    #[ignore]
    async fn latex_builtin() {
        let pdf = latex::render("lualatex", include_str!("../../../assets/doc-templates/invoice.tex"), &hostile(), None, 120).await.unwrap_or_else(|e| panic!("{e}"));
        out("latex", &pdf);
    }

    #[tokio::test]
    #[ignore]
    async fn office_builtin() {
        let docx = office::fill_docx(include_bytes!("../../../assets/doc-templates/invoice.docx"), &hostile()).unwrap();
        let pdf = office::convert("soffice", &docx, "docx", "pdf", 120).await.unwrap_or_else(|e| panic!("{e}"));
        out("office", &pdf);
    }

    const STYLES: [&str; 3] = ["classic", "modern", "minimal"];

    /// The hostile data, plus a record without products and without optional fields.
    fn variants() -> Vec<(&'static str, serde_json::Value)> {
        let mut bare = hostile();
        bare["products"] = serde_json::json!([]);
        bare["has_products"] = serde_json::json!(false);
        bare["taxes"] = serde_json::json!([]);
        bare["totals"]["has_discount"] = serde_json::json!(false);
        bare["totals"]["has_tax"] = serde_json::json!(false);
        for k in ["contact_person", "address", "tax_line", "phone"] {
            bare["client"][k] = serde_json::json!("");
        }
        bare["seller"]["bank_details"] = serde_json::json!("");
        bare["record"]["comments"] = serde_json::json!("");
        vec![("full", hostile()), ("bare", bare)]
    }

    fn example(style: &str, ext: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("assets/doc-templates/invoice-{style}.{ext}"))
    }

    #[test]
    #[ignore]
    fn typst_examples() {
        for style in STYLES {
            let src = std::fs::read_to_string(example(style, "typ")).unwrap();
            for (v, d) in variants() {
                let pdf = typst_engine::render(&src, &d, None).unwrap_or_else(|e| panic!("{style}/{v}: {e}"));
                out(&format!("typst-{style}-{v}"), &pdf);
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn latex_examples() {
        for style in STYLES {
            let src = std::fs::read_to_string(example(style, "tex")).unwrap();
            for (v, d) in variants() {
                let pdf = latex::render("lualatex", &src, &d, None, 180).await.unwrap_or_else(|e| panic!("{style}/{v}: {e}"));
                out(&format!("latex-{style}-{v}"), &pdf);
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn office_examples() {
        for style in STYLES {
            let tpl = std::fs::read(example(style, "docx")).unwrap();
            for (v, d) in variants() {
                let docx = office::fill_docx(&tpl, &d).unwrap_or_else(|e| panic!("{style}/{v}: {e}"));
                let pdf = office::convert("soffice", &docx, "docx", "pdf", 180).await.unwrap_or_else(|e| panic!("{style}/{v}: {e}"));
                out(&format!("office-{style}-{v}"), &pdf);
            }
        }
    }
}
