//! Shared styling-asset loading (#469, #472): the small machinery both font and
//! image resolution build on — the error kind, the single-file loader, and the
//! `(role, path)` → embedded-struct resolver. Kept off the filesystem (the caller
//! supplies `load`) so resolution is testable.

/// Why a styling asset (font or image) couldn't be loaded.
pub(super) enum AssetError {
    Unsupported,
    Read(std::io::Error),
}

/// Load one styling-asset file: look up its (media type, extension) via `media`,
/// then read its bytes via `load`. Shared by font (#469) and image (#472)
/// resolution; the caller maps the error to a domain-specific warning.
pub(super) fn load_asset(
    path: &str,
    media: impl Fn(&str) -> Option<(&'static str, &'static str)>,
    load: &mut impl FnMut(&str) -> std::io::Result<Vec<u8>>,
) -> Result<(&'static str, &'static str, Vec<u8>), AssetError> {
    let (media_type, ext) = media(path).ok_or(AssetError::Unsupported)?;
    let bytes = load(path).map_err(AssetError::Read)?;
    Ok((media_type, ext, bytes))
}

/// Resolve a set of configured `(role, Option<path>)` asset targets into embedded
/// structs, returning them alongside one warning per asset that couldn't be read
/// or had an unsupported type. The shared body of [`super::ExportStyle::resolve_fonts`]
/// and [`super::ExportStyle::resolve_images`] (#504): `media` looks up the (media type,
/// extension); `kind`/`exts` shape the warning text (`unsupported {kind}` lists
/// `{exts}`); `load` reads the bytes; `embed` builds the role-specific struct.
pub(super) fn resolve_assets<Role, Asset, const N: usize>(
    targets: [(Role, Option<String>); N],
    media: impl Fn(&str) -> Option<(&'static str, &'static str)>,
    kind: &str,
    exts: &str,
    mut load: impl FnMut(&str) -> std::io::Result<Vec<u8>>,
    mut embed: impl FnMut(Role, &str, &'static str, &'static str, Vec<u8>) -> Asset,
) -> (Vec<Asset>, Vec<String>) {
    let mut assets = Vec::new();
    let mut warnings = Vec::new();
    for (role, path) in targets {
        let Some(path) = path else { continue };
        match load_asset(&path, &media, &mut load) {
            Ok((media_type, ext, bytes)) => assets.push(embed(role, &path, media_type, ext, bytes)),
            Err(AssetError::Unsupported) => {
                warnings.push(format!("style: unsupported {kind} '{path}' (use {exts})"));
            }
            Err(AssetError::Read(e)) => {
                warnings.push(format!("style: couldn't read {kind} '{path}': {e}"));
            }
        }
    }
    (assets, warnings)
}
