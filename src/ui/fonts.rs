//! Finding the monospace fonts installed on this machine, and handing one to
//! egui.
//!
//! egui only knows the families that have been registered with it, and asking
//! for an unregistered one is not a soft failure — it panics inside glyph
//! measurement. So nothing may name a family that [`install`] has not
//! confirmed; that is the whole reason this module reports success rather than
//! just doing the work.

use std::sync::OnceLock;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// Key the terminal font is registered under. Fixed, so switching fonts
/// replaces the entry instead of accumulating one per family ever chosen.
const TERMINAL_FONT: &str = "nit-terminal-font";

/// The system font list, loaded once.
///
/// Enumerating installed fonts costs on the order of 100 ms, which is fine at
/// startup and not fine every time the Settings combo box is opened — the same
/// reason instance discovery is cached in [`crate::pty::launcher`].
fn database() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// Installed monospace families, sorted, without duplicates.
///
/// Restricted to monospace because a proportional font in a character grid is
/// unusable rather than merely ugly: every column is one cell wide, so the
/// glyphs would not line up with anything.
pub fn monospace_families() -> Vec<String> {
    let mut names: Vec<String> = database()
        .faces()
        .filter(|face| face.monospaced)
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

/// Registers `family` as the terminal font, returning whether it worked.
///
/// An empty name means "egui's bundled monospace", which is always available
/// and needs no work. A name that is not installed, or whose file cannot be
/// read, returns `false` — the caller must then fall back rather than let the
/// name reach a `FontFamily::Name`.
pub fn install(ctx: &Context, family: &str) -> bool {
    if family.is_empty() || family == "monospace" {
        ctx.set_fonts(FontDefinitions::default());
        return true;
    }

    let Some((bytes, index)) = face_data(family) else {
        log::warn!("font family {family:?} is not installed; keeping the built-in monospace");
        return false;
    };

    let mut defs = FontDefinitions::default();
    defs.font_data.insert(
        TERMINAL_FONT.to_owned(),
        FontData {
            font: std::borrow::Cow::Owned(bytes),
            // Collections (.ttc) hold several faces in one file, so the index
            // fontdb reported has to come along or we would install whichever
            // face happens to be first.
            index,
            tweak: Default::default(),
        },
    );
    // Registered under both names: its own, which the terminal asks for, and at
    // the front of Monospace so `ui.code` and the macro previews match the
    // terminal. The bundled font stays behind it as the fallback that covers
    // whatever glyphs the chosen font is missing.
    defs.families
        .entry(FontFamily::Name(family.into()))
        .or_default()
        .insert(0, TERMINAL_FONT.to_owned());
    defs.families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, TERMINAL_FONT.to_owned());

    ctx.set_fonts(defs);
    true
}

/// Bytes and face index of the regular face of `family`.
///
/// The lightest upright face wins. Matching on the family name alone would
/// happily pick Bold or Italic, whichever the enumeration reached first, and
/// the terminal would then render everything bold with no way to undo it.
fn face_data(family: &str) -> Option<(Vec<u8>, u32)> {
    let db = database();
    let wanted = family.to_lowercase();

    let best = db
        .faces()
        .filter(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.to_lowercase() == wanted)
        })
        .min_by_key(|face| {
            let upright = face.style != fontdb::Style::Normal;
            // Distance from Regular, so a family shipping only Light or Medium
            // still resolves instead of being rejected.
            let weight = face.weight.0.abs_diff(fontdb::Weight::NORMAL.0);
            (upright, weight)
        })?;

    let index = best.index;
    let bytes = db.with_face_data(best.id, |data, _| data.to_vec())?;
    Some((bytes, index))
}
