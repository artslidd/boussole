//! Text measurement for layout code that must know label sizes before Slint renders them.
//!
//! The board places arrow labels in Rust (see [`crate::board`]), so it needs the width of a
//! label *before* the renderer lays it out. The widths come from the very same font file the
//! UI embeds (`ui/fonts/BarlowSemiCondensed-SemiBold.ttf`), so the measurement is identical on
//! every platform. Kerning is ignored, which can only over-estimate (labels get a hair of
//! extra padding, never too little).

use std::sync::OnceLock;
use ttf_parser::Face;

static LABEL_FONT: &[u8] = include_bytes!("../ui/fonts/BarlowSemiCondensed-SemiBold.ttf");
static DISPLAY_FONT: &[u8] = include_bytes!("../ui/fonts/YoungSerif-Regular.ttf");

/// Advance width used for glyphs the font does not contain (em fraction).
const FALLBACK_ADVANCE_EM: f64 = 0.55;

fn label_face() -> &'static Face<'static> {
    static FACE: OnceLock<Face<'static>> = OnceLock::new();
    FACE.get_or_init(|| Face::parse(LABEL_FONT, 0).expect("embedded label font is a valid TTF"))
}

fn display_face() -> &'static Face<'static> {
    static FACE: OnceLock<Face<'static>> = OnceLock::new();
    FACE.get_or_init(|| Face::parse(DISPLAY_FONT, 0).expect("embedded display font is a valid TTF"))
}

fn text_width(face: &Face<'_>, text: &str, font_px: f64) -> f64 {
    let units_per_em = f64::from(face.units_per_em());
    text.chars()
        .map(|ch| {
            face.glyph_index(ch)
                .and_then(|glyph| face.glyph_hor_advance(glyph))
                .map_or(FALLBACK_ADVANCE_EM, |advance| {
                    f64::from(advance) / units_per_em
                })
        })
        .sum::<f64>()
        * font_px
}

/// Width in logical pixels of `text` set on one line in the label font (Barlow Semi Condensed
/// SemiBold) at `font_px`.
pub fn label_text_width(text: &str, font_px: f64) -> f64 {
    text_width(label_face(), text, font_px)
}

/// Width in logical pixels of `text` set on one line in the display font (Young Serif) at
/// `font_px`.
pub fn display_text_width(text: &str, font_px: f64) -> f64 {
    text_width(display_face(), text, font_px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_has_no_width() {
        assert_eq!(label_text_width("", 14.0), 0.0);
    }

    #[test]
    fn width_scales_linearly_with_font_size() {
        let small = label_text_width("Strasbourg", 10.0);
        let large = label_text_width("Strasbourg", 20.0);
        assert!((large - 2.0 * small).abs() < 1e-9);
    }

    #[test]
    fn wide_letters_measure_wider_than_narrow_ones() {
        assert!(label_text_width("MMMM", 14.0) > label_text_width("iiii", 14.0));
    }

    #[test]
    fn french_accents_and_separator_are_in_the_font() {
        // A missing glyph would silently use the fallback advance and may render as tofu.
        let face = label_face();
        for ch in "éèêëàâîïôùûüçœÉÈÀÇŒ·-' 0123456789".chars() {
            assert!(face.glyph_index(ch).is_some(), "missing glyph {ch:?}");
        }
    }

    #[test]
    fn display_font_has_the_glyphs_the_dial_needs() {
        let face = display_face();
        for ch in "?éèêëàâîïôùûüçœÉÈÀÇŒ-' ’.!".chars() {
            assert!(face.glyph_index(ch).is_some(), "missing glyph {ch:?}");
        }
        assert!(display_text_width("Besançon", 20.0) > label_text_width("Besançon", 20.0));
    }

    #[test]
    fn typical_label_width_is_plausible() {
        // A semi-condensed 14 px label for a 14-character name is roughly 80–120 px wide.
        let width = label_text_width("Boulogne-sur-Mer", 14.0);
        assert!((70.0..130.0).contains(&width), "width {width}");
    }
}
