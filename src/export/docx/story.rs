//! The short story submission manuscript (#1241): standard short story
//! manuscript format rather than a novel's. Page one carries the author's
//! contact block top-left with the rounded word count at the right, then the
//! title and byline about a third of the way down, and the story begins right
//! under them — no title page, no contents, no chapter heading, no page break.
//! Scene breaks are a centered `#`, a centered "END" closes the story, and the
//! running header reads `Surname / Short title / page`.

use super::body::{heading, para, prose_paragraph, run, wrap_document};
use super::{prose_blocks, running_header, Block, Document, Header, Profile};
use crate::export::{approximate_words, StoryMeta};

/// Single-spaced, so the contact block reads as an address, not manuscript text.
const SINGLE: &str = r#"<w:spacing w:after="0" w:line="240" w:lineRule="auto"/>"#;

/// How far the title sits below the contact block: roughly a third of the page.
const TITLE_DROP: &str = "3600";

/// A title longer than this is cut to its first words for the running header.
const SHORT_TITLE_CHARS: usize = 30;

/// The `word/document.xml` for a short story's submission manuscript.
pub(super) fn story_document_xml(doc: &Document, story: &StoryMeta) -> String {
    let mut body = contact_block(story);
    body.push_str(&para(
        &format!(r#"<w:spacing w:before="{TITLE_DROP}" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#),
        run(&doc.title, false, false, None),
    ));
    if let Some(author) = &doc.author {
        body.push_str(&para(
            r#"<w:spacing w:after="480" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(&format!("by {author}"), false, false, None),
        ));
    }
    let mut sections = String::new();
    for block in doc.blocks() {
        let Block::Chapter(chapter) = block else {
            continue; // a story has no parts
        };
        if doc.is_story_body(chapter) {
            for prose in prose_blocks(&chapter.body) {
                body.push_str(&prose_paragraph(&prose));
            }
        } else {
            // A dedication or afterword follows the story, each on its own page,
            // so nothing pushes the story off page one.
            sections.push_str(&heading(&chapter.title, Profile::Manuscript));
            for prose in prose_blocks(&chapter.body) {
                sections.push_str(&prose_paragraph(&prose));
            }
        }
    }
    body.push_str(&para(
        r#"<w:spacing w:before="480"/><w:jc w:val="center"/>"#,
        run("END", false, false, None),
    ));
    body.push_str(&sections);
    wrap_document(&body, &story_header(doc))
}

/// The contact block: the first line carries the word count right-aligned on a
/// tab stop at the right margin, the rest of the details single-spaced beneath.
/// With no contact details set, the word count stands alone on the right.
fn contact_block(story: &StoryMeta) -> String {
    let count = approximate_words(story.words);
    let mut lines = story.contact.iter();
    let first = lines.next().map_or("", String::as_str);
    let tab_stop = r#"<w:tabs><w:tab w:val="right" w:pos="9360"/></w:tabs>"#;
    let mut out = para(
        &format!("{tab_stop}{SINGLE}"),
        format!(
            "{}<w:r><w:tab/></w:r>{}",
            run(first, false, false, None),
            run(&count, false, false, None)
        ),
    );
    for line in lines {
        out.push_str(&para(SINGLE, run(line, false, false, None)));
    }
    out
}

/// The running header for a story: `Surname / Short title / page`.
pub(super) fn story_header(doc: &Document) -> Header {
    let surname = doc
        .author
        .as_deref()
        .and_then(|a| a.split_whitespace().last());
    running_header(surname, Some(&short_title(&doc.title)), Profile::Manuscript)
}

/// The title as the running header carries it: whole when short, else its
/// leading words up to [`SHORT_TITLE_CHARS`].
fn short_title(title: &str) -> String {
    if title.chars().count() <= SHORT_TITLE_CHARS {
        return title.to_string();
    }
    let mut out = String::new();
    for word in title.split_whitespace() {
        if !out.is_empty() && out.len() + word.len() + 1 > SHORT_TITLE_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::short_title;

    #[test]
    fn a_long_title_is_cut_to_its_leading_words() {
        assert_eq!(short_title("The Drowned Bell"), "The Drowned Bell");
        assert_eq!(
            short_title("What the Lighthouse Keeper Never Told the Harbour Master"),
            "What the Lighthouse Keeper"
        );
    }
}
