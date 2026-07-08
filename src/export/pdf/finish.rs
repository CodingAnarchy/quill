//! Serializing the built [`Pdf`] to file bytes: [`Pdf::finish`] decorates every page
//! (running heads + folios), lays out the object model (catalog, page tree, fonts,
//! images, content streams, bookmark outline), stamps the PDF/X-3 output intent and
//! `/Info` (#692/#817), and hands the objects to [`serialize`]. Split from `mod.rs` to
//! keep both under the size convention (#927); the page/body assembly stays there.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::draw::num;
use super::font::{pdf_text_string, take, GidMap, ObjMap};
use super::images::emit_images;
use super::write::{flate_stream, flatten_outline, remap_glyph_ids, serialize};
use super::{icc, Pdf};

impl Pdf {
    /// Serialize the pages and outline into the bytes of a PDF file.
    pub(super) fn finish(mut self) -> Vec<u8> {
        if self.pages.is_empty() {
            self.start_page();
        }
        let n_pages = self.pages.len();
        // Build the page decorations (running heads + folios) first: they resolve
        // glyphs too, and every used glyph must be recorded before the fonts are
        // subset below.
        let decorations: Vec<String> = (0..n_pages).map(|i| self.decorations(i)).collect();
        let mut objects: ObjMap = BTreeMap::new();
        // 1 catalog, 2 pages, 3 outlines, then fonts, pages+contents, outline items.
        let mut next = 4u32;
        let font_names = self.layout.fonts.objects(&mut next, &mut objects);
        let image_names = emit_images(&self.images, self.layout.pdf_x, &mut next, &mut objects);
        let mut page_obj = Vec::with_capacity(n_pages);
        let mut content_obj = Vec::with_capacity(n_pages);
        for _ in 0..n_pages {
            page_obj.push(next);
            next += 1;
            content_obj.push(next);
            next += 1;
        }
        let outline = std::mem::take(&mut self.outline);
        let mut item_count = 0usize;
        let (first, last) = flatten_outline(
            &outline,
            3,
            &mut next,
            &page_obj,
            self.layout.media_h(),
            &mut objects,
            &mut item_count,
        );

        // Page tree (MediaBox + font resources inherited by every page).
        let kids = page_obj.iter().fold(String::new(), |mut s, id| {
            let _ = write!(s, "{id} 0 R ");
            s
        });
        let xobjects = if image_names.is_empty() {
            String::new()
        } else {
            let entries = image_names.iter().fold(String::new(), |mut s, (name, id)| {
                let _ = write!(s, "/{name} {id} 0 R ");
                s
            });
            format!(" /XObject << {entries}>>")
        };
        let fonts = font_names
            .iter()
            .fold(String::new(), |mut s, (name, id, _)| {
                let _ = write!(s, "/{name} {id} 0 R ");
                s
            });
        let resources = format!("/Resources << /Font << {fonts}>>{xobjects} >>");
        // Subsetting renumbered glyphs, so remap the content-stream glyph ids.
        let maps: Vec<(&str, &GidMap)> = font_names.iter().map(|(name, _, m)| (*name, m)).collect();
        objects.insert(
            2,
            format!(
                "<< /Type /Pages /Kids [ {kids}] /Count {n_pages} /MediaBox [0 0 {} {}] {resources} >>",
                num(self.layout.media_w()),
                num(self.layout.media_h())
            )
            .into_bytes(),
        );
        // The trim (cut) and bleed rectangles a print shop reads (#692). TrimBox is the
        // finished page inset within the media box by the bleed; BleedBox is the full
        // media box. With no bleed the two coincide with the media box, but stating them
        // explicitly keeps the file unambiguous. Emitted on every page.
        let (tx0, ty0, tx1, ty1) = self.layout.trim_box();
        let (bx0, by0, bx1, by1) = self.layout.bleed_box();
        let boxes = format!(
            "/TrimBox [{} {} {} {}] /BleedBox [{} {} {} {}]",
            num(tx0),
            num(ty0),
            num(tx1),
            num(ty1),
            num(bx0),
            num(by0),
            num(bx1),
            num(by1)
        );
        for i in 0..n_pages {
            objects.insert(
                page_obj[i],
                format!(
                    "<< /Type /Page /Parent 2 0 R /Contents {} 0 R {boxes} >>",
                    content_obj[i]
                )
                .into_bytes(),
            );
            let mut content = std::mem::take(&mut self.pages[i]);
            content.push_str(&decorations[i]);
            let content = remap_glyph_ids(&content, &maps);
            // FlateDecode-compress the page content stream (#662); the helper adds
            // `/Filter /FlateDecode` and the compressed `/Length`.
            objects.insert(content_obj[i], flate_stream(content.as_bytes(), ""));
        }
        let outlines = match (first, last) {
            // `item_count == 0` iff the outline was empty, iff `flatten_outline`
            // returned `(None, None)`; matching the ids directly ties the emitted
            // First/Last to their presence with no panic path.
            (Some(first), Some(last)) => format!(
                "<< /Type /Outlines /First {first} 0 R /Last {last} 0 R /Count {item_count} >>"
            ),
            _ => "<< /Type /Outlines /Count 0 >>".to_string(),
        };
        objects.insert(3, outlines.into_bytes());
        // The PDF/X output intent (#817), print interior only: a grayscale
        // `/OutputIntent` naming an embedded ICC destination profile, referenced from
        // the catalog's `/OutputIntents`. The screen proof stays plain. Object ids come
        // off `next` before `/Info` so the intent + ICC stream are numbered first.
        let output_intents = self.layout.pdf_x.then(|| {
            let icc_id = take(&mut next);
            // The ICC profile stream carries `/N 1` (a 1-component, grayscale space) so
            // a reader knows the destination colour space without parsing the profile.
            objects.insert(icc_id, flate_stream(icc::GRAY_ICC, " /N 1"));
            let intent_id = take(&mut next);
            objects.insert(
                intent_id,
                format!(
                    "<< /Type /OutputIntent /S /GTS_PDFX \
                     /OutputConditionIdentifier {ident} /OutputCondition {cond} \
                     /Info {ident} /DestOutputProfile {icc_id} 0 R >>",
                    ident = pdf_text_string(icc::OUTPUT_CONDITION),
                    cond = pdf_text_string(icc::OUTPUT_CONDITION),
                )
                .into_bytes(),
            );
            format!(" /OutputIntents [{intent_id} 0 R]")
        });
        let output_intents = output_intents.unwrap_or_default();
        objects.insert(
            1,
            format!(
                "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R /PageMode /UseOutlines{output_intents} >>"
            )
            .into_bytes(),
        );
        // The document `/Info` dictionary (#692): Title/Author from the manifest, and a
        // Producer identifying Quill. CreationDate is omitted here (the export is
        // deterministic — no wall-clock nondeterminism a test/script would trip on). The
        // print interior also stamps `/GTS_PDFXVersion` + `/GTS_PDFXConformance` (#817),
        // the keys a PDF/X reader keys off to select conformance validation.
        let info_id = take(&mut next);
        let mut info = String::from("<<");
        if !self.doc_title.is_empty() {
            let _ = write!(info, " /Title {}", pdf_text_string(&self.doc_title));
        }
        if !self.doc_author.is_empty() {
            let _ = write!(info, " /Author {}", pdf_text_string(&self.doc_author));
        }
        let _ = write!(info, " /Producer {}", pdf_text_string("Quill"));
        if self.layout.pdf_x {
            let _ = write!(
                info,
                " /GTS_PDFXVersion {ver} /GTS_PDFXConformance {ver}",
                ver = pdf_text_string(icc::PDFX_VERSION)
            );
        }
        info.push_str(" >>");
        objects.insert(info_id, info.into_bytes());
        serialize(&objects, Some(info_id))
    }
}
