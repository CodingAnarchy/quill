//! The in-app font browser (#494): browse the curated open-licensed library
//! ([`crate::fonts`]), download a family into the project, and set it for the
//! designed exports' `[style]` `body_font` / `heading_font` — all without leaving
//! Quill or hand-editing a path. Acquisition + selection only; the chosen path
//! flows into the unchanged `resolve_fonts` → EPUB/PDF embedding pipeline.
//!
//! Layout: downloads land in the project's `fonts/` directory as
//! `fonts/<slug>.ttf`, with the family's `OFL` license beside it as
//! `fonts/<slug>.LICENSE.txt` so the license travels with the font. The relative
//! `fonts/<slug>.ttf` path is what the manifest stores and what the resolver reads
//! (`project.root.join(path)`), so a downloaded font embeds exactly like a
//! hand-specified one — and the existing "point at a file path" option keeps
//! working, untouched.
//!
//! Network access is opt-in and happens only on an explicit download. The actual
//! fetch is gated out of test builds (#531) so the suite can never reach the wire;
//! tests drive the browse/select/apply logic and the `[style]` splice with a
//! hermetic scratch project, asserting the manifest exactly as a hand-typed one.

use super::App;
use crate::config;
use crate::fonts::{self, FontFamily};
use crate::mode::{Command, FontTarget, PaletteItem};
use crate::project::{read_existing, Project};

/// The project subdirectory downloaded fonts live in — also the prefix of the
/// relative path stored in `[style]`, so the export resolver finds them.
const FONTS_DIR: &str = "fonts";

/// Guidance shown when an author tries to set a font on a multi-book series
/// (#1105). A series manifest carries no per-book `[style]`, so the font can't
/// apply to the active book; this points at the path that does work (a standalone
/// novel's own project) instead of a hand-edit that wouldn't take effect. The
/// font file the author downloaded stays in `fonts/`, so re-applying it in a
/// standalone project is instant and offline.
const SERIES_FONT_GUIDANCE: &str =
    "In-app fonts apply to a standalone novel — a multi-book series doesn't carry \
     per-book [style] fonts yet. Manage the book as its own project to set its \
     font (the downloaded file stays in fonts/).";

impl App {
    /// Open the curated font library (#494): a palette of every open-licensed
    /// family, each row showing its name, style, and a sample so the author can
    /// choose. Enter on a family opens the body/heading role picker.
    pub(crate) fn open_font_browser(&mut self) {
        let items = fonts::FAMILIES
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let desc = format!("{} — “{}”", f.style, f.sample);
                PaletteItem::new(f.name, desc, Command::PickFontRole(i))
            })
            .collect();
        self.open_palette("Fonts — OFL (open-licensed)", items);
    }

    /// After a family is chosen, ask which slot it fills (#494): the body text, the
    /// headings, or both. Each choice downloads (if needed) and applies.
    pub(crate) fn open_font_role_picker(&mut self, index: usize) {
        if fonts::FAMILIES.get(index).is_none() {
            self.mode = self.editing_mode();
            return;
        }
        let row = |label: &str, desc: &str, target| {
            PaletteItem::new(label, desc, Command::ApplyFont(index, target))
        };
        let items = vec![
            row("Body text", "Set this as body_font", FontTarget::Body),
            row("Headings", "Set this as heading_font", FontTarget::Heading),
            row(
                "Body and headings",
                "Set both body_font and heading_font",
                FontTarget::Both,
            ),
        ];
        self.open_palette("Apply font to…", items);
    }

    /// Download the chosen family (if it isn't already cached in the project) and
    /// set it for `target` in the project's `[style]` (#494). The download is
    /// synchronous (a single regular-weight `.ttf` is small, low-tens-to-hundreds
    /// of KB) and only reaches the network on a real, non-test build. On success
    /// the relative `fonts/<slug>.ttf` path is spliced into the manifest, so the
    /// next export embeds it through the existing pipeline.
    pub(crate) fn apply_font(&mut self, index: usize, target: FontTarget) {
        self.mode = self.editing_mode();
        let Some(family) = fonts::FAMILIES.get(index) else {
            return;
        };
        // A multi-book series carries no per-book [style] (#1105): show the clean
        // guidance up front rather than downloading a font we couldn't apply and
        // then reporting it as a write failure.
        if self.project.book_count() > 1 {
            self.message = Some(SERIES_FONT_GUIDANCE.to_string());
            return;
        }
        let root = self.project.root.clone();
        let rel_path = match ensure_font_downloaded(&root, family) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some(format!("Couldn't download {}: {e}", family.name));
                return;
            }
        };
        match self.set_style_font(target, &rel_path) {
            Ok(()) => {
                self.message = Some(format!(
                    "{} downloaded and set as {}. It embeds on the next EPUB / print-PDF export.",
                    family.name,
                    target_label(target)
                ));
            }
            Err(e) => {
                self.message = Some(format!(
                    "Saved {} but couldn't update [style]: {e}",
                    family.name
                ));
            }
        }
    }

    /// Splice the `[style]` font path(s) for `target` into the project manifest and
    /// the in-memory style, so a re-open and the next export both see them (#494).
    /// Reuses the manifest splicer the goals setter uses (#550), leaving the rest of
    /// `book.toml` byte-for-byte. Standalone novels only — a multi-book series
    /// carries no per-book `[style]`, so the font setter degrades to a clear message
    /// pointing at the path that works (a standalone novel's own project) rather
    /// than a hand-edit that wouldn't take effect (#1105).
    fn set_style_font(&mut self, target: FontTarget, rel_path: &str) -> std::io::Result<()> {
        if self.project.book_count() > 1 {
            return Err(std::io::Error::other(SERIES_FONT_GUIDANCE));
        }
        // Update the in-memory style first, so a preview reflects it immediately.
        let style = self
            .project
            .manifest
            .style
            .get_or_insert_with(Default::default);
        if matches!(target, FontTarget::Body | FontTarget::Both) {
            style.body_font = Some(rel_path.to_string());
        }
        if matches!(target, FontTarget::Heading | FontTarget::Both) {
            style.heading_font = Some(rel_path.to_string());
        }

        // Persist the path(s) as a targeted splice into `[style]`, preserving the
        // rest of the manifest (mirrors how #550 writes `[goals]`).
        let path = Project::manifest_path(&self.project.root);
        // A failed read must abort: splicing `[style]` onto an empty base would
        // overwrite the whole manifest while reporting success (#565).
        let mut text = read_existing(&path)?;
        let quoted = toml::Value::String(rel_path.to_string()).to_string();
        if matches!(target, FontTarget::Body | FontTarget::Both) {
            text = config::set_value(&text, "style", "body_font", &quoted);
        }
        if matches!(target, FontTarget::Heading | FontTarget::Both) {
            text = config::set_value(&text, "style", "heading_font", &quoted);
        }
        crate::atomic::write(&path, text.as_bytes())
    }
}

/// Ensure `family`'s fonts (and its license) are in the project's `fonts/` dir,
/// downloading them if absent, and return the relative `fonts/<slug>.ttf` path the
/// manifest stores. The upright `[wght]` file lands at `<slug>.ttf` and the
/// true-italic file beside it at `<slug>-italic.ttf` — the sibling the export
/// resolver picks up so italic and bold-italic body prose render in a real italic
/// face (#638). A cache hit on the upright file (already on disk) makes no network
/// call, so re-applying a font is instant and offline.
fn ensure_font_downloaded(root: &std::path::Path, family: &FontFamily) -> std::io::Result<String> {
    let slug = family.slug();
    let rel_path = format!("{FONTS_DIR}/{slug}.ttf");
    let ttf_path = root.join(&rel_path);
    if ttf_path.exists() {
        return Ok(rel_path);
    }
    let dir = root.join(FONTS_DIR);
    std::fs::create_dir_all(&dir)?;
    let bytes = fetch(family.ttf_url)?;
    std::fs::write(&ttf_path, &bytes)?;
    // The true-italic sibling is best-effort: a fetch failure leaves the upright
    // font fully usable (the writer synthesizes the slope), so it doesn't abort.
    if let Ok(italic) = fetch(family.italic_ttf_url) {
        let _ = std::fs::write(dir.join(format!("{slug}-italic.ttf")), italic);
    }
    // The license travels with the font; a license fetch failure is non-fatal (the
    // font is still usable) but worth surfacing only if the font itself failed.
    if let Ok(license) = fetch(family.license_url) {
        let license_path = dir.join(format!("{slug}.LICENSE.txt"));
        let _ = std::fs::write(license_path, license);
    }
    Ok(rel_path)
}

/// A font file's read cap (#494): variable-weight `.ttf`s run a few MB; 16 MiB is
/// comfortably above any single face while still bounding a runaway response.
#[cfg(not(test))]
const MAX_FONT_BYTES: u64 = 16 * 1024 * 1024;

/// Fetch `url`'s bytes over HTTPS (#494). The one outbound call in the font flow,
/// gated out of test builds so the suite can never reach the network (#531).
#[cfg(not(test))]
fn fetch(url: &str) -> std::io::Result<Vec<u8>> {
    let mut res = ureq::get(url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| std::io::Error::other(format!("request failed: {e}")))?;
    let status = res.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(std::io::Error::other(format!(
            "download failed (HTTP {status})"
        )));
    }
    res.body_mut()
        .with_config()
        .limit(MAX_FONT_BYTES)
        .read_to_vec()
        .map_err(|e| std::io::Error::other(format!("read failed: {e}")))
}

/// Test stub: refuse the network so the suite stays hermetic (#531). Tests that
/// exercise apply-with-a-font seed the cache file first, so they hit the
/// cache-hit path and never reach here.
#[cfg(test)]
fn fetch(_url: &str) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::other(
        "outbound network is disabled in tests (#531)",
    ))
}

/// A human label for the `[style]` slot a font was set into, for the status line.
const fn target_label(target: FontTarget) -> &'static str {
    match target {
        FontTarget::Body => "the body font",
        FontTarget::Heading => "the heading font",
        FontTarget::Both => "the body and heading font",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app_scratch_dir, open_test_app, TempProject};
    use crate::mode::{Action, Mode};

    /// A one-chapter project on disk whose manifest a font test inspects.
    fn font_app(name: &str) -> (App, TempProject) {
        let dir = app_scratch_dir(name);
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "first chapter\n").unwrap();
        let app = open_test_app(&dir);
        (app, TempProject::new(dir))
    }

    /// Seed the cache file a family would download, so `apply_font` hits the
    /// cache-hit path and never calls the (test-stubbed) network.
    fn seed_cache(root: &std::path::Path, family: &FontFamily) -> String {
        let rel = format!("{FONTS_DIR}/{}.ttf", family.slug());
        let path = root.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A minimal TrueType signature; the resolver only needs bytes + a .ttf ext.
        std::fs::write(&path, b"\x00\x01\x00\x00stub").unwrap();
        rel
    }

    #[test]
    fn the_browse_fonts_action_is_registered() {
        assert!(
            Action::ALL.iter().any(|a| a.name() == "fonts:browse"),
            "fonts:browse is in the registry"
        );
        assert!(matches!(
            Action::from_name("fonts:browse"),
            Some(Action::BrowseFonts)
        ));
    }

    #[test]
    fn browsing_lists_every_curated_family_with_a_sample() {
        let (mut app, _dir) = font_app("fonts-browse");
        app.open_font_browser();
        let Mode::Palette(p) = &app.mode else {
            panic!("font browser opens a palette");
        };
        assert_eq!(p.items.len(), fonts::FAMILIES.len());
        // Each row shows enough to choose: a name, a style, and a sample.
        for (item, family) in p.items.iter().zip(fonts::FAMILIES) {
            assert_eq!(item.label, family.name);
            assert!(item.desc.contains(family.style), "{}", item.desc);
            assert!(item.desc.contains(family.sample), "{}", item.desc);
        }
    }

    #[test]
    fn the_role_picker_offers_body_heading_and_both() {
        let (mut app, _dir) = font_app("fonts-roles");
        app.open_font_role_picker(0);
        let Mode::Palette(p) = &app.mode else {
            panic!("role picker opens a palette");
        };
        assert_eq!(p.items.len(), 3, "body / heading / both");
    }

    #[test]
    fn applying_a_cached_font_splices_the_style_path_into_book_toml() {
        let (mut app, dir) = font_app("fonts-apply-body");
        let family = &fonts::FAMILIES[0];
        let rel = seed_cache(&dir, family);

        app.apply_font(0, FontTarget::Body);

        // In-memory style carries the path immediately.
        let style = app.project.manifest.style.clone().expect("style set");
        assert_eq!(style.body_font.as_deref(), Some(rel.as_str()));
        assert!(
            style.heading_font.is_none(),
            "body-only doesn't set headings"
        );

        // And it persisted to the `[style]` table of book.toml, re-opening identically.
        let toml = std::fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(toml.contains("[style]"), "{toml}");
        assert!(toml.contains(&format!("body_font = \"{rel}\"")), "{toml}");
        let reopened = Project::open(&*dir).unwrap();
        assert_eq!(
            reopened.manifest.style.unwrap().body_font.as_deref(),
            Some(rel.as_str())
        );
    }

    #[test]
    fn applying_to_both_sets_body_and_heading() {
        let (mut app, dir) = font_app("fonts-apply-both");
        let family = &fonts::FAMILIES[1];
        let rel = seed_cache(&dir, family);

        app.apply_font(1, FontTarget::Both);

        let style = app.project.manifest.style.clone().expect("style set");
        assert_eq!(style.body_font.as_deref(), Some(rel.as_str()));
        assert_eq!(style.heading_font.as_deref(), Some(rel.as_str()));
        let toml = std::fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(
            toml.contains(&format!("heading_font = \"{rel}\"")),
            "{toml}"
        );
    }

    #[test]
    fn an_app_set_font_reads_back_identical_to_a_hand_typed_one() {
        // Set via the app (cache-hit path, no network)...
        let (mut app, dir) = font_app("fonts-roundtrip-app");
        let family = &fonts::FAMILIES[0];
        let rel = seed_cache(&dir, family);
        app.apply_font(0, FontTarget::Body);
        let from_app = Project::open(&*dir).unwrap().manifest.style.unwrap();

        // ...and the same path hand-typed into a fresh manifest's [style].
        let hand = app_scratch_dir("fonts-roundtrip-hand");
        std::fs::write(
            Project::manifest_path(&hand),
            format!(
                "title = \"Book\"\n\n[style]\nbody_font = \"{rel}\"\n\n\
                 [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n"
            ),
        )
        .unwrap();
        std::fs::write(hand.join("one.md"), "x\n").unwrap();
        let from_hand = Project::open(&hand).unwrap().manifest.style.unwrap();
        let _hand = TempProject::new(hand);

        assert_eq!(from_app, from_hand, "app-set font == hand-typed font");
    }

    #[test]
    fn re_applying_a_cached_font_makes_no_network_call() {
        // `fetch` errors in tests; a cache hit must avoid it entirely, so a second
        // apply still succeeds without touching the network.
        let (mut app, dir) = font_app("fonts-cache-hit");
        seed_cache(&dir, &fonts::FAMILIES[0]);
        app.apply_font(0, FontTarget::Body);
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("downloaded and set"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn ensure_download_reports_network_error_when_uncached() {
        // No seeded cache → the (test-stubbed) fetch refuses, surfaced as an error
        // rather than a panic, proving the suite can't reach the wire (#531).
        let dir = app_scratch_dir("fonts-no-net");
        let err = ensure_font_downloaded(&dir, &fonts::FAMILIES[0]).unwrap_err();
        assert!(err.to_string().contains("network is disabled"), "{err}");
        let _dir = TempProject::new(dir);
    }

    /// A one-book-per-`[[books]]` series project, for the series-guidance test.
    fn series_app(name: &str) -> (App, TempProject) {
        let dir = app_scratch_dir(name);
        std::fs::create_dir_all(dir.join("b1")).unwrap();
        std::fs::create_dir_all(dir.join("b2")).unwrap();
        std::fs::write(dir.join("b1/one.md"), "alpha\n").unwrap();
        std::fs::write(dir.join("b2/one.md"), "beta\n").unwrap();
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"The Saga\"\n\n\
             [[books]]\ntitle = \"Book One\"\n\n\
             [[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n\
             [[books]]\ntitle = \"Book Two\"\n\n\
             [[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n",
        )
        .unwrap();
        let app = open_test_app(&dir);
        (app, TempProject::new(dir))
    }

    #[test]
    fn a_series_font_set_guides_to_a_standalone_without_downloading() {
        // A multi-book series carries no per-book [style] (#1105): applying a font
        // must guide to a standalone project up front — not download (which the
        // test-stubbed fetch would refuse) and not read as a write failure. The
        // font is uncached, so if apply_font tried to fetch it, the network stub
        // would surface a "network is disabled" error instead of the guidance.
        let (mut app, dir) = series_app("fonts-series");
        app.apply_font(0, FontTarget::Both);
        let msg = app.message.as_deref().unwrap_or("");
        assert!(msg.contains("standalone"), "{msg:?}");
        assert!(!msg.contains("isn't supported yet"), "{msg:?}");
        assert!(!msg.contains("per book by hand"), "{msg:?}");
        assert!(!msg.contains("network is disabled"), "no download: {msg:?}");
        // No [style] was written to the series manifest.
        let toml = std::fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(
            !toml.contains("[style]"),
            "series manifest untouched:\n{toml}"
        );
    }
}
