//! Manuscript export (#7). A [`Document`] is the project compiled into a single,
//! ordered structure — book metadata, then parts and chapters with their
//! epigraphs and prose, in reading order. This module is the shared layer the
//! format writers build on: [`to_markdown`] renders one to a single Markdown file
//! (#190); the Word / Scrivener / Vellum writers (#191–#193) consume the same
//! `Document`.

use crate::frontmatter::Epigraph;

/// A project compiled for export: the book's metadata and its ordered contents.
pub struct Document {
    pub title: String,
    pub subtitle: Option<String>,
    pub author: Option<String>,
    pub series: Option<String>,
    /// The book epigraph, rendered at the start.
    pub epigraph: Option<Epigraph>,
    /// Parts and chapters in reading order (front matter → body → back matter).
    pub items: Vec<Item>,
}

/// One element of the compiled book.
pub enum Item {
    /// A part heading, emitted before the chapters it groups.
    Part(String),
    /// A chapter or front/back-matter section: its title, optional epigraph, and
    /// prose body (front matter already stripped).
    Chapter {
        title: String,
        epigraph: Option<Epigraph>,
        body: String,
    },
}

/// Render a [`Document`] as a single Markdown document (#190): a title block,
/// then each part/chapter under a heading with its epigraph set apart as a
/// block quote. Clean, pandoc-friendly Markdown — the manuscript's native form.
pub fn to_markdown(doc: &Document) -> String {
    let mut out = String::new();
    if let Some(series) = &doc.series {
        out.push_str(series);
        out.push_str("\n\n");
    }
    out.push_str(&format!("# {}\n\n", doc.title));
    if let Some(subtitle) = &doc.subtitle {
        out.push_str(&format!("*{subtitle}*\n\n"));
    }
    if let Some(author) = &doc.author {
        out.push_str(&format!("by {author}\n\n"));
    }
    if let Some(epigraph) = &doc.epigraph {
        out.push_str(&epigraph_block(epigraph));
    }

    for item in &doc.items {
        match item {
            Item::Part(name) => out.push_str(&format!("## {name}\n\n")),
            Item::Chapter { title, epigraph, body } => {
                out.push_str(&format!("## {title}\n\n"));
                if let Some(epigraph) = epigraph {
                    out.push_str(&epigraph_block(epigraph));
                }
                let body = body.trim();
                if !body.is_empty() {
                    out.push_str(body);
                    out.push_str("\n\n");
                }
            }
        }
    }
    out
}

/// Drop a chapter body's leading `# {title}` line when it just repeats the
/// chapter title (the compiled heading already carries it), so the export doesn't
/// show the title twice. A body whose first heading differs from the title is
/// left untouched — only a true duplicate is removed. Mirrors the outline pane's
/// `redundant_title_heading`.
pub(crate) fn strip_repeated_title(body: &str, title: &str) -> String {
    let trimmed = body.trim_start_matches('\n');
    let Some(rest) = trimmed.strip_prefix("# ") else {
        return body.to_string();
    };
    let (heading, after) = rest.split_once('\n').unwrap_or((rest, ""));
    if heading.trim() == title.trim() {
        after.trim_start_matches('\n').to_string()
    } else {
        body.to_string()
    }
}

/// An epigraph as a Markdown block quote — each line prefixed `> `, the
/// attribution on its own dimmed line — set apart from the surrounding prose.
fn epigraph_block(epigraph: &Epigraph) -> String {
    let mut out = String::new();
    for line in epigraph.text.lines() {
        if line.is_empty() {
            out.push_str(">\n");
        } else {
            out.push_str(&format!("> {line}\n"));
        }
    }
    if let Some(attribution) = &epigraph.attribution {
        out.push_str(&format!(">\n> — {attribution}\n"));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
        Epigraph { text: text.to_string(), attribution: by.map(str::to_string) }
    }

    fn doc() -> Document {
        Document {
            title: "Red State Earth".into(),
            subtitle: Some("a novel".into()),
            author: Some("A. Writer".into()),
            series: Some("The Cycle".into()),
            epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
            items: vec![
                Item::Chapter {
                    title: "Prologue".into(),
                    epigraph: None,
                    body: "Before it all.".into(),
                },
                Item::Part("Part One".into()),
                Item::Chapter {
                    title: "The Find".into(),
                    epigraph: Some(epigraph("A line.", None)),
                    body: "The locket *gleamed*.".into(),
                },
            ],
        }
    }

    #[test]
    fn renders_the_book_header_and_epigraph() {
        let md = to_markdown(&doc());
        assert!(md.starts_with("The Cycle\n\n# Red State Earth\n\n*a novel*\n\nby A. Writer\n\n"));
        assert!(md.contains("> In the beginning.\n>\n> — Anon\n"), "{md}");
    }

    #[test]
    fn renders_items_in_order_with_headings() {
        let md = to_markdown(&doc());
        let prologue = md.find("## Prologue").unwrap();
        let part = md.find("## Part One").unwrap();
        let chapter = md.find("## The Find").unwrap();
        assert!(prologue < part && part < chapter, "reading order preserved:\n{md}");
        // The chapter epigraph renders before its body; prose is verbatim.
        let find = &md[chapter..];
        assert!(find.find("> A line.").unwrap() < find.find("The locket *gleamed*.").unwrap(), "{md}");
    }

    #[test]
    fn strips_a_body_heading_only_when_it_repeats_the_title() {
        // A leading `# Title` that matches is dropped (the compiled heading has it).
        assert_eq!(strip_repeated_title("# The Find\n\nProse.\n", "The Find"), "Prose.\n");
        // A differing heading is kept (don't lose the author's text).
        assert_eq!(
            strip_repeated_title("# Prologue\n\nBefore.\n", "A Prologue"),
            "# Prologue\n\nBefore.\n"
        );
        // No leading heading → untouched.
        assert_eq!(strip_repeated_title("Just prose.\n", "Title"), "Just prose.\n");
    }

    #[test]
    fn a_multiline_epigraph_quotes_every_line() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: Some(epigraph("Roses are red\n\nViolets are blue", None)),
            items: vec![],
        };
        let md = to_markdown(&d);
        assert!(md.contains("> Roses are red\n>\n> Violets are blue\n"), "{md}");
    }
}
