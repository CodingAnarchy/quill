//! Export every book under a corpus folder (`tests/export-corpus`) to EPUB,
//! DOCX, and PDF with the core's own export code (#1301), so validators can
//! check the files. A series is exported as its omnibus and as its open book.
//!
//!     cargo run -p quill-core --example export_corpus -- <corpus> <out>
//!
//! Opening a book writes its lock under `.quill/`, so point it at a copy of the
//! corpus (`scripts/check-exports.sh` does).

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use quill_core::export::assemble::{self, Assembly, Format};
use quill_core::export::{docx, epub};
use quill_core::project::Project;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, corpus, out] = args.as_slice() else {
        eprintln!("usage: export_corpus <corpus> <out>");
        return ExitCode::from(2);
    };
    match run(Path::new(corpus), Path::new(out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("export_corpus: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(corpus: &Path, out: &Path) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut books: Vec<_> = fs::read_dir(corpus)
        .map_err(|e| format!("{}: {e}", corpus.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("book.toml").exists())
        .collect();
    books.sort();
    for dir in books {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let project = Project::open(&dir).map_err(|e| format!("{name}: {e}"))?;
        let (style, warnings) = assemble::resolve_style(&project);
        for warning in warnings {
            eprintln!("{name}: {warning}");
        }
        let assembly = Assembly::new(&project, None, &style);
        let doc = assembly.document(None);
        for format in [Format::Epub, Format::Manuscript, Format::Pdf] {
            let (bytes, warnings) = assemble::render(&project, &doc, format);
            for warning in warnings {
                eprintln!("{name}: {warning}");
            }
            let bytes = bytes.map_err(|e| format!("{name}: {e}"))?;
            write(out, &format!("{name}.{}", format.extension()), &bytes)?;
        }
        if project.book_count() > 1 {
            let docs = assembly.series_documents();
            let series = project.series_title();
            let author = project.manifest.author.as_deref();
            let stamp = quill_core::date::utc_timestamp();
            let omnibus = epub::to_epub_series(&docs, series, author, &stamp, &style)
                .map_err(|e| format!("{name}: {e}"))?;
            write(out, &format!("{name}-omnibus.epub"), &omnibus)?;
            let omnibus = docx::to_docx_series(&docs, series, author, docx::Profile::Manuscript)
                .map_err(|e| format!("{name}: {e}"))?;
            write(out, &format!("{name}-omnibus.docx"), &omnibus)?;
        }
        println!("exported {name}");
    }
    Ok(())
}

fn write(out: &Path, file: &str, bytes: &[u8]) -> Result<(), String> {
    let path = out.join(file);
    fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}
