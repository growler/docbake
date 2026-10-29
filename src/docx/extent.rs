//! Image extents in EMU (914400 EMU = 1 in).
//!
//! [`length`] reads the value of a `width` or `height` attribute. [`extent`]
//! gives the extent of an image from its pixel size and these lengths.

use super::params::{split_number, Scaled};

/// EMU per inch.
const EMU_PER_INCH: u64 = 914_400;

/// The largest attribute length in EMU (22 in).
const MAX_LENGTH: i128 = 22 * EMU_PER_INCH as i128;

/// Parses the value `s` of a `width` or `height` attribute and gives it in
/// EMU. `percent_of` is the length of `100%` and `em` is the length of
/// `1em`, both in EMU. A number without a unit is in pixels at 96 DPI.
/// `auto` gives `None`. The parser ignores whitespace before and after the
/// value. A length that is not more than 0 or is more than 22 in is an
/// error. A length less than 1 EMU gives 1 EMU.
pub(super) fn length(s: &str, percent_of: u64, em: u64) -> Result<Option<u64>, String> {
    let s = s.trim();
    if s == "auto" {
        return Ok(None);
    }
    if s.is_empty() {
        return Err("missing value; use a number and a unit such as 5cm".to_owned());
    }
    let Some((number, unit)) = split_number(s) else {
        return Err(format!(
            "invalid number in '{s}'; use a number and a unit such as 5cm"
        ));
    };
    let (num, den) = match unit {
        "%" => (i128::from(percent_of), 100),
        "px" | "" => (9525, 1),
        "pt" => (12_700, 1),
        "cm" => (360_000, 1),
        "mm" => (36_000, 1),
        "in" => (i128::from(EMU_PER_INCH), 1),
        "em" => (i128::from(em), 1),
        other => {
            return Err(format!(
                "unknown unit '{other}' in '{s}'; use %, px, pt, cm, mm, in or em"
            ))
        }
    };
    let emu = Scaled::new(number, num, den)?;
    match emu.signum() {
        -1 => return Err(format!("negative length '{s}'")),
        0 => return Err(format!("length '{s}' must be more than 0")),
        _ => {}
    }
    if !emu.within(0, MAX_LENGTH) {
        return Err(format!("length '{s}' is too large; the maximum is 22in"));
    }
    Ok(Some(emu.round().max(1) as u64))
}

/// Gives the extent `(cx, cy)` in EMU of an image with the pixel size `px`
/// and `ppi` pixels per inch.
///
/// - `width` and `height` set: the extent is these lengths.
/// - Only one of them set: the other side keeps the aspect ratio of `px`.
/// - Neither set: the extent is the pixel size at `ppi`.
///
/// An extent wider or taller than `max` becomes smaller to fit in `max`.
/// Both sides change by the same factor. Each side is rounded half away
/// from zero and is at least 1 EMU.
pub(super) fn extent(
    px: (u32, u32),
    ppi: u32,
    width: Option<u64>,
    height: Option<u64>,
    max: (u64, u64),
) -> (u32, u32) {
    let (w_px, h_px) = (u128::from(px.0.max(1)), u128::from(px.1.max(1)));
    // The extent is (w / d, h / d).
    let (w, h, d) = match (width, height) {
        (Some(w), Some(h)) => (u128::from(w), u128::from(h), 1),
        (Some(w), None) => (u128::from(w) * w_px, u128::from(w) * h_px, w_px),
        (None, Some(h)) => (u128::from(h) * w_px, u128::from(h) * h_px, h_px),
        (None, None) => (
            w_px * u128::from(EMU_PER_INCH),
            h_px * u128::from(EMU_PER_INCH),
            u128::from(ppi.max(1)),
        ),
    };
    let (max_w, max_h) = (u128::from(max.0), u128::from(max.1));
    let (cx, cy) = if w <= max_w * d && h <= max_h * d {
        (round(w, d), round(h, d))
    } else if max_w * h <= max_h * w {
        (max_w, round(h * max_w, w))
    } else {
        (round(w * max_h, h), max_h)
    };
    let side = |v: u128| u32::try_from(v.max(1)).unwrap_or(u32::MAX);
    (side(cx), side(cy))
}

/// Gives `n / d` rounded half away from zero.
fn round(n: u128, d: u128) -> u128 {
    (2 * n + d) / (2 * d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text width and the text height of the blank theme.
    const TEXT: (u64, u64) = (6_480_175, 9_251_950);

    /// 100% and 1em for the tests.
    const PERCENT_OF: u64 = 1_000_000;
    const EM: u64 = 139_700;

    fn parse(s: &str) -> Result<Option<u64>, String> {
        length(s, PERCENT_OF, EM)
    }

    #[test]
    fn length_units() {
        for (text, emu) in [
            ("1in", 914_400),
            ("2.5cm", 900_000),
            ("10mm", 360_000),
            ("12pt", 152_400),
            ("96px", 914_400),
            ("96", 914_400),
            ("1.5em", 209_550),
            ("50%", 500_000),
            ("33.3333333%", 333_333),
            ("0.5px", 4763),
            ("0.000001mm", 1),
            ("  5cm\t", 1_800_000),
            ("22in", 20_116_800),
            // 29 fraction digits, the most that the digit limit permits.
            ("1.00000000000000000000000000000%", 10_000),
            ("1.00000000000000000000000000000cm", 360_000),
        ] {
            assert_eq!(parse(text), Ok(Some(emu)), "{text}");
        }
        assert_eq!(parse("auto"), Ok(None));
        assert_eq!(parse(" auto "), Ok(None));
    }

    #[test]
    fn length_rejects() {
        for (text, message) in [
            ("", "missing value; use a number and a unit such as 5cm"),
            (
                "abc",
                "invalid number in 'abc'; use a number and a unit such as 5cm",
            ),
            (
                ".5cm",
                "invalid number in '.5cm'; use a number and a unit such as 5cm",
            ),
            (
                "3xx",
                "unknown unit 'xx' in '3xx'; use %, px, pt, cm, mm, in or em",
            ),
            (
                "10tw",
                "unknown unit 'tw' in '10tw'; use %, px, pt, cm, mm, in or em",
            ),
            (
                "5CM",
                "unknown unit 'CM' in '5CM'; use %, px, pt, cm, mm, in or em",
            ),
            (
                "50% + 1cm",
                "unknown unit '% + 1cm' in '50% + 1cm'; use %, px, pt, cm, mm, in or em",
            ),
            (
                "5 cm",
                "unknown unit ' cm' in '5 cm'; use %, px, pt, cm, mm, in or em",
            ),
            (
                "Auto",
                "invalid number in 'Auto'; use a number and a unit such as 5cm",
            ),
            ("0", "length '0' must be more than 0"),
            ("0.0cm", "length '0.0cm' must be more than 0"),
            ("-0", "length '-0' must be more than 0"),
            ("-1cm", "negative length '-1cm'"),
            (
                "22.001in",
                "length '22.001in' is too large; the maximum is 22in",
            ),
            ("2012%", "length '2012%' is too large; the maximum is 22in"),
            (
                "1234567890123456789012345678901cm",
                "number '1234567890123456789012345678901' is too large; use at most 30 digits",
            ),
            (
                "1.000000000000000000000000000000%",
                "number '1.000000000000000000000000000000' is too large; use at most 30 digits",
            ),
            (
                "1.000000000000000000000000000000cm",
                "number '1.000000000000000000000000000000' is too large; use at most 30 digits",
            ),
            (
                "22.0000000000000000000000000001in",
                "length '22.0000000000000000000000000001in' is too large; the maximum is 22in",
            ),
        ] {
            assert_eq!(parse(text), Err(message.to_owned()), "{text}");
        }
    }

    #[test]
    fn natural_extent_uses_pixels_per_inch() {
        assert_eq!(
            extent((400, 100), 96, None, None, TEXT),
            (3_810_000, 952_500)
        );
        assert_eq!(
            extent((288, 144), 144, None, None, TEXT),
            (1_828_800, 914_400)
        );
        assert_eq!(extent((3, 1), 192, None, None, TEXT), (14_288, 4763));
    }

    #[test]
    fn one_length_keeps_aspect_ratio() {
        assert_eq!(
            extent((400, 100), 96, Some(1_800_000), None, TEXT),
            (1_800_000, 450_000)
        );
        assert_eq!(
            extent((400, 100), 96, None, Some(914_400), TEXT),
            (3_657_600, 914_400)
        );
        assert_eq!(extent((3, 1), 96, Some(10), None, TEXT), (10, 3));
        assert_eq!(extent((3, 2), 96, Some(1), None, TEXT), (1, 1));
        assert_eq!(extent((1000, 1), 96, Some(10), None, TEXT), (10, 1));
    }

    #[test]
    fn two_lengths_are_exact() {
        assert_eq!(
            extent((400, 100), 96, Some(720_000), Some(1_080_000), TEXT),
            (720_000, 1_080_000)
        );
    }

    #[test]
    fn extent_fits_in_max() {
        // Wider than the text width.
        assert_eq!(
            extent((2000, 400), 96, None, None, TEXT),
            (6_480_175, 1_296_035)
        );
        assert_eq!(
            extent((400, 100), 96, Some(10_800_000), None, TEXT),
            (6_480_175, 1_620_044)
        );
        // Taller than the text height. 2312987.5 rounds up.
        assert_eq!(
            extent((100, 400), 96, None, Some(10_800_000), TEXT),
            (2_312_988, 9_251_950)
        );
        // Both lengths set: the distorted ratio stays.
        assert_eq!(
            extent((1, 1), 96, Some(12_960_350), Some(1_000_000), TEXT),
            (6_480_175, 500_000)
        );
        // A side of exactly the maximum fits.
        assert_eq!(
            extent((1, 1), 96, Some(TEXT.0), Some(TEXT.1), TEXT),
            (6_480_175, 9_251_950)
        );
    }

    #[test]
    fn extent_of_extreme_sizes() {
        let max = (u64::from(u32::MAX), u64::from(u32::MAX));
        assert_eq!(extent((u32::MAX, 1), 96, None, None, max), (u32::MAX, 1));
        assert_eq!(
            extent((u32::MAX, u32::MAX), 96, None, None, TEXT),
            (6_480_175, 6_480_175)
        );
        assert_eq!(extent((0, 0), 96, None, None, TEXT), (9525, 9525));
        assert_eq!(extent((0, 5), 96, Some(100), None, TEXT), (100, 500));
    }
}
