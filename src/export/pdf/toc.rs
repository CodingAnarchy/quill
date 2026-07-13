//! The printed table-of-contents page (#603): planning the Contents layout,
//! reserving its front-matter pages before the body is laid out, and filling in the
//! titles + right-aligned folios once the folios are final. The [`Pdf`] methods here
//! (`plan_contents`, `reserve_contents`, `fill_contents`) and the free helpers that
//! shape the entries live in this sibling of `mod.rs` to keep both under the size
//! convention (#716); the entry points and the rest of the builder stay in `mod.rs`.

use super::wrap::{segments, words_of, wrap_lines, Hyphenate, Seg};
use super::write::OutlineItem;
use super::{
    Document, FontStyle, Pdf, Run, TextStyle, BODY_LEADING, BODY_SIZE, HEADING_LEADING,
    HEADING_SIZE,
};

// The printed table-of-contents page (#603): its heading, list rhythm, and the gutter
// reserved at the right of each line for the right-aligned folio. The gutter is sized
// for a four-digit folio so a title wraps to the same width whether the reservation
// pass or the fill pass measures it — the actual (shorter) folio is right-aligned in it.
const CONTENTS_TITLE: &str = "Contents";
const TOC_LEADING: f32 = BODY_LEADING * 1.4;
const TOC_FOLIO_GUTTER: f32 = 32.0;

/// The Contents titles a book contributes, in reading order, each flagged as a part
/// heading (named part) or a plain entry (front/back/chapter). Used to size the
/// reserved Contents page(s) before the body — and the folios — are laid out; the
/// order matches [`book_outline`], so [`toc_entries`] pairs titles to folios.
pub(super) fn doc_toc_titles(doc: &Document) -> Vec<(String, bool)> {
    let mut titles: Vec<(String, bool)> =
        doc.front.iter().map(|c| (c.title.clone(), false)).collect();
    for part in &doc.parts {
        if let Some(name) = &part.name {
            titles.push((name.clone(), true));
        }
        titles.extend(part.chapters.iter().map(|c| (c.title.clone(), false)));
    }
    titles.extend(doc.back.iter().map(|c| (c.title.clone(), false)));
    titles
}

/// One planned Contents line: where it lands (TOC-relative page + baseline), the
/// wrapped title segments to draw, and — on the first line of an entry — which entry's
/// folio to right-align beside it. Positions depend only on the titles, so the reserve
/// and fill passes plan identically (see [`Pdf::plan_contents`]).
pub(super) struct TocLine {
    pub(super) page: usize,
    y: f32,
    runs: Vec<Seg>,
    size: f32,
    pub(super) folio_entry: Option<usize>,
}

/// The reserved Contents section: the page index it starts at, how many pages it spans,
/// and the planned lines (folios filled in later).
pub(super) struct TocPlan {
    pub(super) start: usize,
    pub(super) lines: Vec<TocLine>,
}

/// Borrow drawable segments as `glyph_block`'s `(text, style, heading)` tuples.
fn seg_refs(segs: &[Seg]) -> impl Iterator<Item = (&str, FontStyle, bool)> {
    segs.iter().map(|s| (s.text.as_str(), s.style, s.heading))
}

/// Flatten the built outline into per-entry folios, depth-first so a part precedes the
/// chapters nested beneath it — the printed reading order, matching [`doc_toc_titles`].
/// A part heading (`is_part`) groups its chapters and carries no folio (`None`) — even
/// with no chapters nested yet; an entry's folio is its starting page index (the
/// printed label is index + 1).
pub(super) fn toc_entries(items: &[OutlineItem]) -> Vec<Option<usize>> {
    let mut out = Vec::new();
    for item in items {
        // Part-ness is explicit (`is_part`), not inferred from the children — an
        // empty named part is still a divider that carries no folio.
        out.push(if item.is_part { None } else { Some(item.page) });
        out.extend(toc_entries(&item.children));
    }
    out
}

impl Pdf {
    /// Plan the Contents page(s): wrap each title (heading first, then every entry)
    /// into lines paginated across TOC pages, returning where each line lands. The
    /// layout depends only on the titles (part vs. entry) — the folio gutter is a
    /// fixed reserve — so the reservation pass and the fill pass agree on the page
    /// count and line positions, and the fill pass can attach the actual folios.
    fn plan_contents(&self, titles: &[(String, bool)]) -> Vec<TocLine> {
        let col = self.layout.text_w();
        let title_w = col - TOC_FOLIO_GUTTER;
        let top = self.layout.text_top();
        let bottom = self.layout.text_bottom();
        let mut lines = Vec::new();
        let mut page = 0usize;
        let mut y = top;
        // Wrap `text` (bold headings) at `size`/`leading` into `width`, advancing y and
        // paginating, tagging the first line of the block with `entry` for its folio.
        let emit = |lines: &mut Vec<TocLine>,
                    page: &mut usize,
                    y: &mut f32,
                    text: &str,
                    bold: bool,
                    size: f32,
                    leading: f32,
                    width: f32,
                    entry: Option<usize>| {
            let words = words_of(&[Run::new(text, TextStyle::from_flags(bold, false))], false);
            let wrapped = wrap_lines(&self.layout, &words, size, Hyphenate::Off, |_| width);
            for (i, (line, _w)) in wrapped.into_iter().enumerate() {
                if *y - leading < bottom {
                    *page += 1;
                    *y = top;
                }
                lines.push(TocLine {
                    page: *page,
                    y: *y,
                    runs: segments(&line),
                    size,
                    // The folio rides the first line of the entry (i == 0).
                    folio_entry: entry.filter(|_| i == 0),
                });
                *y -= leading;
            }
        };
        // The "Contents" heading, then a gap.
        emit(
            &mut lines,
            &mut page,
            &mut y,
            CONTENTS_TITLE,
            true,
            HEADING_SIZE,
            HEADING_LEADING * 1.6,
            col,
            None,
        );
        for (idx, (title, part)) in titles.iter().enumerate() {
            if *part {
                y = TOC_LEADING.mul_add(-0.5, y); // set a part heading apart from prior chapters
            }
            let size = if *part { BODY_SIZE + 1.0 } else { BODY_SIZE };
            emit(
                &mut lines,
                &mut page,
                &mut y,
                title,
                *part,
                size,
                TOC_LEADING,
                title_w,
                Some(idx),
            );
        }
        lines
    }

    /// Reserve the front-matter Contents page(s) (#603): plan the layout, open the
    /// section (recto start + blank verso like any other), and push that many empty
    /// pages so the body that follows lands on its true folios. Returns the plan for
    /// [`fill_contents`]. No-op (returns `None`) when the book has no sections.
    pub(super) fn reserve_contents(&mut self, titles: &[(String, bool)]) -> Option<TocPlan> {
        if titles.is_empty() {
            return None;
        }
        let lines = self.plan_contents(titles);
        let pages = lines.iter().map(|l| l.page).max().unwrap_or(0) + 1;
        let start = self.open_section();
        for _ in 1..pages {
            self.start_page();
        }
        Some(TocPlan { start, lines })
    }

    /// Fill the reserved Contents page(s) with the planned lines, drawing each title
    /// and the right-aligned folio of the entry it heads (the folio label matches the
    /// page decoration's roman-front/Arabic-body scheme, #651). Runs after the body so
    /// folios are final.
    pub(super) fn fill_contents(&mut self, plan: Option<TocPlan>, folios: &[Option<usize>]) {
        let Some(plan) = plan else { return };
        debug_assert_eq!(
            plan.lines
                .iter()
                .filter(|l| l.folio_entry.is_some())
                .count(),
            folios.len(),
            "every Contents entry got a planned line"
        );
        let col = self.layout.text_w();
        for line in &plan.lines {
            let idx = plan.start + line.page;
            let left = self.layout.left(idx);
            let mut block = self.glyph_block(left, line.y, line.size, seg_refs(&line.runs));
            // The right-aligned folio on the entry's first line, labeled under the
            // same scheme the page decoration uses (#651).
            if let Some(label) = line
                .folio_entry
                .and_then(|i| folios.get(i).copied().flatten())
                .and_then(|folio| self.folio_label(folio))
            {
                let w = self
                    .layout
                    .measure(&label, line.size, FontStyle::REGULAR, false);
                let x = left + col - w;
                block.push_str(&self.text_op(x, line.y, &label, line.size));
            }
            self.pages[idx].push_str(&block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::toc_entries;
    use crate::export::pdf::write::OutlineItem;

    // `OutlineItem` is `pub(super)` in the sibling `write` module; from this test
    // module the qualified path resolves it within the `pdf` module.

    /// A part divider carries no folio even with no chapters nested beneath it: an
    /// empty named part (`Part{name:Some, chapters:[]}`) becomes an `is_part`
    /// [`OutlineItem`] with empty children, and [`toc_entries`] must still map it to
    /// `None` (no folio on its Contents line) rather than treating the empty-children
    /// item as a plain folio-bearing entry (the #1118 regression).
    #[test]
    fn empty_named_part_carries_no_folio() {
        let items = vec![
            OutlineItem::new("Front".into(), 3),
            OutlineItem::part("Empty Part".into(), 7),
            OutlineItem::new("Back".into(), 9),
        ];
        assert_eq!(
            toc_entries(&items),
            vec![Some(3), None, Some(9)],
            "the empty named part gets no folio (None); plain entries keep theirs"
        );
    }

    /// A part with chapters nested beneath it: the part itself is folio-less, and its
    /// chapters follow in reading order with their own folios — confirming the
    /// explicit `is_part` flag, not `children.is_empty()`, decides part-ness.
    #[test]
    fn populated_part_is_folioless_but_chapters_keep_folios() {
        let mut part = OutlineItem::part("Part One".into(), 5);
        part.children.push(OutlineItem::new("Ch 1".into(), 6));
        part.children.push(OutlineItem::new("Ch 2".into(), 12));
        assert_eq!(toc_entries(&[part]), vec![None, Some(6), Some(12)]);
    }
}
