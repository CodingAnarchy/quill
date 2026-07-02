//! The [`Pdf`] builder's designed front-matter pages: the full-bleed cover (#582),
//! the title page, the copyright page (#583), and the standalone part/book divider
//! pages (with a series book's reprinted copyright, #711). Split from `mod.rs` to
//! keep both under the size convention (#716); the chapter/body layout, the Contents
//! machinery, and the public entry points stay in their own modules.

use super::draw::num;
use super::{
    copyright_lines, Align, Epigraph, ImageRole, Pdf, PubMeta, BODY_LEADING, BODY_SIZE,
    EPIGRAPH_LEADING, EPIGRAPH_SIZE, HEADING_SIZE, TITLE_SIZE,
};

impl Pdf {
    /// The book cover (#582), when a cover image is configured: a full-page page 0
    /// holding the cover image, scaled to cover the whole media box — the trim plus any
    /// bleed allowance (#652) — with the aspect ratio preserved and centered, so
    /// full-bleed art reaches the page edge with no white border (POD needs this).
    /// Overflow past the media box is cropped by the page (`clip_media` clips the
    /// content to the media box). The page carries no running head or folio. No-op when
    /// no cover is set, leaving the output unchanged. The cover scales to fill the
    /// media-box width, so its effective resolution is fixed here; the print DPI-floor
    /// check (#696) is the same `media_w()` used below and is surfaced to the exporter
    /// by [`pdf_dpi_warnings`] rather than mutating the emitted page.
    pub(super) fn cover_page(&mut self) {
        let Some((name, w, h)) = self
            .image(ImageRole::Cover)
            .map(|i| (i.name.clone(), i.width as f32, i.height as f32))
        else {
            return;
        };
        self.start_page();
        // No running head or folio on the cover.
        self.meta.last_mut().unwrap().blank = true;
        let (page_w, page_h) = (self.layout.media_w(), self.layout.media_h());
        // Scale to cover the whole media box (bleed included), keeping the aspect ratio,
        // and center; overflow is cropped so there's no white margin at the trim edge.
        let scale = (page_w / w).max(page_h / h);
        let (dw, dh) = (w * scale, h * scale);
        let x = (page_w - dw) / 2.0;
        let y = (page_h - dh) / 2.0;
        // Clip to the media box so cover-fill overflow (a differing aspect ratio) is
        // cropped at the page edge rather than spilling past it.
        self.cur().push_str(&format!(
            "q 0 0 {} {} re W n {} 0 0 {} {} {} cm /{name} Do Q\n",
            num(page_w),
            num(page_h),
            num(dw),
            num(dh),
            num(x),
            num(y)
        ));
    }

    /// The book title page: series, title, subtitle, author, and the book epigraph,
    /// centered and set a third of the way down.
    pub(super) fn title_page(
        &mut self,
        series: Option<&str>,
        title: &str,
        subtitle: Option<&str>,
        author: Option<&str>,
        epigraph: Option<&Epigraph>,
    ) {
        self.open_section();
        if self.image(ImageRole::Title).is_some() {
            // A title image sits at the top, with the text block below it (#490).
            self.y = self.layout.text_top() - BODY_LEADING;
            self.place_image(
                ImageRole::Title,
                self.layout.text_w(),
                self.layout.page_h * 0.3,
                BODY_LEADING * 2.0,
            );
        } else {
            self.y = self.layout.trim_y(0.62);
        }
        if let Some(s) = series {
            self.centered(s, false, false, false, BODY_SIZE, BODY_LEADING * 2.0);
        }
        // The book title uses the heading font (#638).
        self.centered(title, true, false, true, TITLE_SIZE, TITLE_SIZE * 1.4);
        if let Some(s) = subtitle {
            self.centered(
                s,
                false,
                true,
                false,
                EPIGRAPH_SIZE + 3.0,
                BODY_LEADING * 1.6,
            );
        }
        if let Some(a) = author {
            self.centered(
                &format!("by {a}"),
                false,
                false,
                false,
                BODY_SIZE,
                BODY_LEADING,
            );
        }
        if let Some(o) = self
            .bits
            .title_ornament
            .clone()
            .filter(|o| self.layout.renders(o))
        {
            self.centered(&o, false, false, false, HEADING_SIZE, BODY_LEADING);
        }
        if let Some(e) = epigraph {
            self.y -= BODY_LEADING * 3.0;
            self.epigraph(e);
        }
    }

    /// The copyright page (#583), when publishing metadata warrants one: the rights
    /// line, publisher, publication date, and ISBN, centered low on a fresh page.
    /// Slots into the conventional front-matter order after the title page (cover →
    /// title → copyright → body). No-op when no metadata is set, leaving the output
    /// unchanged. Not a bookmark.
    pub(super) fn copyright_page(&mut self, publishing: &PubMeta) {
        let lines = copyright_lines(publishing);
        if lines.is_empty() {
            return;
        }
        self.open_section();
        // Set the block low on the page, the conventional copyright-page placement.
        self.y = self.layout.trim_y(0.3);
        for line in lines {
            self.centered(&line, false, false, false, EPIGRAPH_SIZE, EPIGRAPH_LEADING);
        }
    }

    /// A standalone divider page (a part or a series book): the title (with the given
    /// alignment), and an optional ornament glyph beneath it.
    pub(super) fn divider_page(
        &mut self,
        title: &str,
        size: f32,
        ornament: Option<&str>,
        align: Align,
    ) -> usize {
        let page = self.open_section();
        self.y = self.layout.trim_y(0.5);
        self.heading_line(title, size, size * 1.4, align);
        if let Some(o) = ornament.filter(|o| self.layout.renders(o)) {
            self.centered(o, false, false, false, HEADING_SIZE, BODY_LEADING);
        }
        page
    }

    /// A series book's own copyright block (#711), set low on its just-opened
    /// divider page so a later book's rights/ISBN aren't silently dropped in an
    /// omnibus — the omnibus's front-matter copyright page stays series-level (see
    /// [`render_series`]). No-op when the book has no publishing metadata, leaving
    /// the divider page unchanged. Not a bookmark.
    pub(super) fn divider_copyright(&mut self, publishing: &PubMeta) {
        let lines = copyright_lines(publishing);
        if lines.is_empty() {
            return;
        }
        // Set the block low on the divider page, the conventional copyright placement.
        self.y = self.layout.trim_y(0.25);
        for line in lines {
            self.centered(&line, false, false, false, EPIGRAPH_SIZE, EPIGRAPH_LEADING);
        }
    }
}
