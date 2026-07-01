//! The curated open-licensed font library (#494): a small, static table of
//! well-known SIL OFL families an author can browse, download, and apply to the
//! designed exports' `[style]` `body_font` / `heading_font` without hunting down
//! files or hand-editing paths. Acquisition + selection only — the embedding
//! pipeline (`export::style::resolve_fonts` → the EPUB/PDF writers) is unchanged;
//! choosing a font just writes the relative `fonts/<file>.ttf` path it resolves.
//!
//! Every family here is offered **only** because it ships under the SIL Open Font
//! License, which permits embedding and redistribution — so an author can't pick a
//! font they aren't licensed to ship inside their book. The license text is
//! downloaded alongside the `.ttf` (see [`crate::app::fonts`]), so it travels with
//! the file the export embeds.
//!
//! The download URLs point at the upstream `google/fonts` repository's raw
//! `ofl/<family>/` directory — the canonical home of the OFL families and their
//! `OFL.txt`. Network access happens only on an explicit download and is gated out
//! of test builds; this module is pure data + lookup, fully unit-testable offline.

/// One curated open-licensed font family (#494): everything the browser shows and
/// the downloader needs. `style` and `sample` give the author enough to choose;
/// `ttf_url` / `license_url` are fetched on an explicit download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontFamily {
    /// Display name shown in the picker (e.g. "EB Garamond").
    pub name: &'static str,
    /// A short style descriptor for the picker's second column (e.g.
    /// "Serif · classic book face").
    pub style: &'static str,
    /// A one-line pangram-ish sample so the author gets a feel for the face
    /// without a glyph preview the terminal can't render in the real typeface.
    pub sample: &'static str,
    /// Raw URL of the upright `.ttf` to embed. Every curated family ships this as a
    /// variable `[wght]`/`[opsz,wght]` file, so its regular *and* bold weights come
    /// from one file — the PDF pins the weight per style and the EPUB `@font-face`
    /// carries a `400 700` weight range (#638).
    pub ttf_url: &'static str,
    /// Raw URL of the true-italic `.ttf` (the `-Italic[wght]` file), embedded as the
    /// `<slug>-italic.ttf` sibling so italic and bold-italic body prose render in a
    /// real italic face rather than the bundled fallback (#638).
    pub italic_ttf_url: &'static str,
    /// Raw URL of the family's `OFL.txt` license, downloaded alongside the font so
    /// the license travels with the file.
    pub license_url: &'static str,
}

impl FontFamily {
    /// A filesystem-safe slug for this family (`EB Garamond` → `eb-garamond`),
    /// used for the cached file name and to look a family back up by slug. Lowercase
    /// ASCII alphanumerics; every run of anything else collapses to a single `-`.
    pub fn slug(&self) -> String {
        slugify(self.name)
    }
}

/// Turn a family name into a filesystem-safe slug. Pure, so the cache layout and
/// the slug lookup share one rule.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The curated library, in the order shown to authors. Kept small but real: a
/// handful of well-known OFL families spanning the looks a novel interior wants —
/// classic and modern serifs for body text, sturdy sans and display faces for
/// headings. Every URL points at the `google/fonts` `ofl/` tree, the canonical
/// OFL distribution.
pub const FAMILIES: &[FontFamily] = &[
    FontFamily {
        name: "EB Garamond",
        style: "Serif · classic book face",
        sample: "The quick brown fox jumps over the lazy dog.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/ebgaramond/EBGaramond%5Bwght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/ebgaramond/EBGaramond-Italic%5Bwght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/ebgaramond/OFL.txt",
    },
    FontFamily {
        name: "Libre Baskerville",
        style: "Serif · readable, traditional",
        sample: "Sphinx of black quartz, judge my vow.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/librebaskerville/LibreBaskerville%5Bwght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/librebaskerville/LibreBaskerville-Italic%5Bwght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/librebaskerville/OFL.txt",
    },
    FontFamily {
        name: "Lora",
        style: "Serif · contemporary, warm",
        sample: "Pack my box with five dozen liquor jugs.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/lora/Lora%5Bwght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/lora/Lora-Italic%5Bwght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/lora/OFL.txt",
    },
    FontFamily {
        name: "Merriweather",
        style: "Serif · sturdy for long text",
        sample: "How vexingly quick daft zebras jump!",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/merriweather/Merriweather%5Bopsz,wdth,wght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/merriweather/Merriweather-Italic%5Bopsz,wdth,wght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/merriweather/OFL.txt",
    },
    FontFamily {
        name: "Source Serif 4",
        style: "Serif · clean, modern",
        sample: "The five boxing wizards jump quickly.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/sourceserif4/SourceSerif4%5Bopsz,wght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/sourceserif4/SourceSerif4-Italic%5Bopsz,wght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/sourceserif4/OFL.txt",
    },
    FontFamily {
        name: "Crimson Pro",
        style: "Serif · old-style, elegant",
        sample: "Jackdaws love my big sphinx of quartz.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/crimsonpro/CrimsonPro%5Bwght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/crimsonpro/CrimsonPro-Italic%5Bwght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/crimsonpro/OFL.txt",
    },
    FontFamily {
        name: "Inter",
        style: "Sans · modern UI / headings",
        sample: "Waltz, bad nymph, for quick jigs vex.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/inter/Inter%5Bopsz,wght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/inter/Inter-Italic%5Bopsz,wght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/inter/OFL.txt",
    },
    FontFamily {
        name: "Montserrat",
        style: "Sans · geometric, for headings",
        sample: "The jay, pig, fox, zebra and my wolves quack!",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/montserrat/Montserrat%5Bwght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/montserrat/Montserrat-Italic%5Bwght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/montserrat/OFL.txt",
    },
    FontFamily {
        name: "Cormorant Garamond",
        style: "Display serif · for title pages",
        sample: "Bright vixens jump; dozy fowl quack.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/cormorantgaramond/CormorantGaramond%5Bwght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/cormorantgaramond/CormorantGaramond-Italic%5Bwght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/cormorantgaramond/OFL.txt",
    },
    FontFamily {
        name: "Playfair Display",
        style: "Display serif · high-contrast",
        sample: "Quick zephyrs blow, vexing daft Jim.",
        ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/playfairdisplay/PlayfairDisplay%5Bwght%5D.ttf",
        italic_ttf_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/playfairdisplay/PlayfairDisplay-Italic%5Bwght%5D.ttf",
        license_url: "https://raw.githubusercontent.com/google/fonts/main/ofl/playfairdisplay/OFL.txt",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_is_filesystem_safe_and_collapses_runs() {
        assert_eq!(slugify("EB Garamond"), "eb-garamond");
        assert_eq!(slugify("Source Serif 4"), "source-serif-4");
        assert_eq!(slugify("Cormorant  Garamond!"), "cormorant-garamond");
        assert_eq!(slugify("  spaced  "), "spaced");
    }

    #[test]
    fn every_curated_family_is_well_formed() {
        assert!(!FAMILIES.is_empty(), "the library has real entries");
        for f in FAMILIES {
            assert!(!f.name.is_empty());
            assert!(!f.style.is_empty());
            assert!(!f.sample.is_empty());
            // Only OFL families are offered, fetched from the canonical OFL tree,
            // and the license always travels with the font.
            assert!(
                f.ttf_url.contains("/ofl/") && f.ttf_url.ends_with(".ttf"),
                "{} has an OFL .ttf url: {}",
                f.name,
                f.ttf_url
            );
            // The true-italic file (#638): an OFL `.ttf`, distinct from the upright,
            // and named for an italic face so it embeds as the `-italic` sibling.
            assert!(
                f.italic_ttf_url.contains("/ofl/") && f.italic_ttf_url.ends_with(".ttf"),
                "{} has an OFL italic .ttf url: {}",
                f.name,
                f.italic_ttf_url
            );
            assert!(
                f.italic_ttf_url.contains("Italic") && f.italic_ttf_url != f.ttf_url,
                "{} italic url is a distinct italic face: {}",
                f.name,
                f.italic_ttf_url
            );
            assert!(
                f.license_url.ends_with("OFL.txt"),
                "{} ships its OFL license: {}",
                f.name,
                f.license_url
            );
            assert!(!f.slug().is_empty());
        }
    }

    #[test]
    fn slugs_are_unique_so_cache_files_dont_collide() {
        let mut slugs: Vec<String> = FAMILIES.iter().map(|f| f.slug()).collect();
        slugs.sort();
        let before = slugs.len();
        slugs.dedup();
        assert_eq!(before, slugs.len(), "every family slug is unique");
    }
}
