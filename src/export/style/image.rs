//! Embedded images for the designed exports (#472, #582, #818): the [`ImageRole`]
//! placement, the resolved [`EmbeddedImage`] file (embedded verbatim, no decode),
//! and the media-type helper the resolver in [`super`] uses to turn a `[style]`
//! image path into embedded bytes.

/// Where an [`EmbeddedImage`] is placed in a designed export (#472).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRole {
    SceneBreak,
    ChapterHeading,
    Title,
    /// The book cover (#582) — the first spine page in the EPUB, a full page 0 in
    /// the PDF.
    Cover,
}

impl ImageRole {
    /// The in-package file slug for this role (`images/<slug>.<ext>`).
    pub(super) const fn slug(self) -> &'static str {
        match self {
            Self::SceneBreak => "scene-break",
            Self::ChapterHeading => "chapter-heading",
            Self::Title => "title",
            Self::Cover => "cover",
        }
    }
}

/// An image file embedded into a designed export (#472): its in-package href, the
/// OPF manifest media type, and the bytes (embedded verbatim — no decode/re-encode,
/// so no image-codec dependency). Resolved by the app from the `[style]` paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedImage {
    pub role: ImageRole,
    pub href: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
    /// Author-supplied alt text (#818): `Some(text)` is a content image described
    /// for assistive tech; `None` declares the image *decorative*, so the writer
    /// emits the empty-alt + `role="presentation"` convention rather than a bare
    /// empty `alt` on a content image.
    pub alt: Option<String>,
}

/// The OPF media type and extension for an image path, or `None` for an
/// unsupported type. The EPUB-core raster/vector image formats.
pub(super) fn image_media_type(path: &str) -> Option<(&'static str, &'static str)> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ("image/png", "png"),
        "jpg" => ("image/jpeg", "jpg"),
        "jpeg" => ("image/jpeg", "jpeg"),
        "gif" => ("image/gif", "gif"),
        "svg" => ("image/svg+xml", "svg"),
        "webp" => ("image/webp", "webp"),
        _ => return None,
    })
}
