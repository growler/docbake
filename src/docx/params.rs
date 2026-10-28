//! Style parameters for the DOCX renderer.
//!
//! A theme sets the DOCX look in a YAML document. [`DocxParams::from_yaml`]
//! reads this document and gives the values in OOXML units: twips,
//! half-points, eighths of a point and RGB colours. It resolves colour names
//! and font roles. Each error starts with the key path, for example
//! `styles.Heading1.color: unknown colour 'acent'`.
//!
//! A length must have a unit: `pt`, `cm`, `mm`, `in` or `tw` (twips).
//! 1 pt = 20 tw, 1 cm = 567 tw, 1 mm = 56.7 tw, 1 in = 1440 tw. The loader
//! uses exact decimal arithmetic and rounds a fraction of a twip half away
//! from zero.
//!
//! Each custom scalar type reads the YAML value as text in a serde visitor
//! and reports its error from that visitor. Thus serde_yaml adds the full key
//! path to the error.

use std::collections::btree_map::Entry;
use std::collections::{hash_map, BTreeMap, HashMap, HashSet};
use std::fmt;
use std::marker::PhantomData;

use anyhow::{anyhow, bail, Result};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;

use super::stylesheet::{self, RESERVED_STYLES};

/// The largest length in twips (22 in).
const MAX_TWIPS: i128 = 31680;

/// The largest number of digits in a number.
const MAX_DIGITS: usize = 30;

/// The IDs of the character styles. All other styles are paragraph styles.
const CHARACTER_STYLES: &[&str] = &[
    "InlineCode",
    "Hyperlink",
    "FootnoteReference",
    "Strong",
    "Emphasis",
];

/// The DOCX style parameters of a theme, with all references resolved.
///
/// The styles keep their `based_on` links. The loader does not copy
/// inherited properties into a style.
#[derive(Debug)]
pub(crate) struct DocxParams {
    pub page: Page,
    pub logo: Logo,
    /// Font role (for example `body`, `heading`, `mono`) to font name.
    pub fonts: BTreeMap<String, String>,
    pub table: TableParams,
    #[allow(dead_code)] // The renderer does not read the list parameters yet.
    pub lists: Lists,
    /// Style ID to style properties.
    pub styles: BTreeMap<String, Style>,
}

impl DocxParams {
    /// Parses a YAML style document and resolves its references.
    pub(crate) fn from_yaml(text: &str) -> Result<Self> {
        let raw: RawParams = serde_yaml::from_str(text)?;
        raw.resolve()
    }
}

/// Page size and margins.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
    pub size: PageSize,
    pub margins: PageMargins,
}

impl Page {
    /// Gives the width of the text area in twips: the page width minus the
    /// left and right margins.
    pub(crate) fn text_width(&self) -> u32 {
        let (width, _) = self.size.dimensions();
        width - self.margins.left.0 - self.margins.right.0
    }
}

/// The heights of the theme logo. The width keeps the aspect ratio of the
/// image.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Logo {
    /// The logo height on the title page.
    pub title_height: Twips,
    /// The logo height in the page header.
    pub header_height: Twips,
}

/// Page margins. `header` and `footer` are the distances from the page edge.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageMargins {
    pub top: Twips,
    pub bottom: Twips,
    pub left: Twips,
    pub right: Twips,
    pub header: Twips,
    pub footer: Twips,
}

/// Paper size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PageSize {
    A4,
    Letter,
}

impl PageSize {
    /// Gives the width and the height in twips.
    pub(crate) fn dimensions(self) -> (u32, u32) {
        match self {
            PageSize::A4 => (11906, 16838),
            PageSize::Letter => (12240, 15840),
        }
    }
}

/// Table parameters.
#[derive(Debug)]
pub(crate) struct TableParams {
    pub cell_margins: CellMargins,
    /// The border of all edges and inside lines.
    pub border: Border,
    /// The format of the header row.
    pub header: TableHeader,
}

/// Table cell margins.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CellMargins {
    pub top: Twips,
    pub bottom: Twips,
    pub left: Twips,
    pub right: Twips,
}

/// The format of the table header row.
#[derive(Debug)]
pub(crate) struct TableHeader {
    pub bold: bool,
    pub fill: Rgb,
}

/// List parameters. Each array has one item for each list level.
#[allow(dead_code)] // The renderer does not read the list parameters yet.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lists {
    /// The indent step for each level.
    pub indent: Twips,
    /// The hanging indent of the list marker.
    pub hanging: Twips,
    #[serde(deserialize_with = "bullets")]
    pub bullets: [String; 6],
    pub numbers: [NumberFormat; 6],
}

/// Number format of a numbered list level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NumberFormat {
    Decimal,
    LowerLetter,
    UpperLetter,
    LowerRoman,
    UpperRoman,
}

/// Paragraph alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Align {
    Left,
    Center,
    Right,
    Justify,
}

/// The type of a style. [`CHARACTER_STYLES`] are character styles. All
/// other styles are paragraph styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StyleKind {
    Paragraph,
    Character,
}

impl StyleKind {
    fn of(id: &str) -> Self {
        if CHARACTER_STYLES.contains(&id) {
            StyleKind::Character
        } else {
            StyleKind::Paragraph
        }
    }

    /// Gives the `w:type` value of the style.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            StyleKind::Paragraph => "paragraph",
            StyleKind::Character => "character",
        }
    }
}

impl fmt::Display for StyleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The properties of one style. `None` means that the style does not set
/// the property.
#[derive(Debug)]
pub(crate) struct Style {
    pub kind: StyleKind,
    /// The ID of the parent style. It is a key of [`DocxParams::styles`].
    pub based_on: Option<String>,
    /// The font name (resolved from the font role).
    pub font: Option<String>,
    pub size: Option<HalfPoints>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub color: Option<Rgb>,
    pub align: Option<Align>,
    pub before: Option<Twips>,
    pub after: Option<Twips>,
    pub line: Option<LineSpacing>,
    /// The left indent.
    pub indent: Option<Twips>,
    pub keep_next: Option<bool>,
    pub fill: Option<Rgb>,
    pub borders: Borders,
    pub hyphenate: Option<bool>,
    pub char_spacing: Option<SignedTwips>,
    /// The smallest font size that gets kerning.
    pub kern: Option<HalfPoints>,
    pub proofing: Option<bool>,
    pub snap_to_grid: Option<bool>,
    pub contextual_spacing: Option<bool>,
}

/// Paragraph borders for each side.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Borders {
    pub top: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    pub right: Option<Border>,
}

/// One single-line border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Border {
    pub width: EighthPoints,
    pub color: Rgb,
    /// The distance between the border and the text.
    pub space: BorderSpace,
}

/// A length in twips (1/20 pt). It is not negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Twips(pub u32);

/// A length in twips that can be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SignedTwips(pub i32);

/// A font size in half-points. YAML gives it in `pt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HalfPoints(pub u32);

/// A border width in eighths of a point. YAML gives it in `pt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EighthPoints(pub u8);

/// A border distance in whole points. YAML gives it in `pt`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BorderSpace(pub u8);

/// Line spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineSpacing {
    /// A multiple of the single line height, in 1/240 of a line.
    Auto(u32),
    /// An exact line height.
    Exact(Twips),
}

/// An RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb(pub [u8; 3]);

/// Writes the colour as six uppercase hex digits, for example `EE721C`.
impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b] = self.0;
        write!(f, "{r:02X}{g:02X}{b:02X}")
    }
}

// ---------------------------------------------------------------------------
// YAML document before resolution
// ---------------------------------------------------------------------------

type Colors = BTreeMap<String, Rgb>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParams {
    page: Page,
    logo: Logo,
    #[serde(deserialize_with = "fonts_map")]
    fonts: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "colors_map")]
    colors: Colors,
    table: RawTable,
    lists: Lists,
    #[serde(deserialize_with = "styles_map")]
    styles: BTreeMap<String, RawStyle>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    cell_margins: CellMargins,
    border: RawTableBorder,
    header: RawTableHeader,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTableBorder {
    width: EighthPoints,
    color: ColorRef,
    space: BorderSpace,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTableHeader {
    bold: bool,
    fill: ColorRef,
}

/// A style border before colour resolution. `S` is the type of `space`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBorder<S> {
    width: EighthPoints,
    color: ColorRef,
    #[serde(default)]
    space: S,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStyle {
    #[serde(default, deserialize_with = "present")]
    based_on: Option<String>,
    #[serde(default, deserialize_with = "present")]
    font: Option<String>,
    #[serde(default, deserialize_with = "present")]
    size: Option<HalfPoints>,
    #[serde(default, deserialize_with = "present")]
    bold: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    italic: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    underline: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    color: Option<ColorRef>,
    #[serde(default, deserialize_with = "present")]
    align: Option<Align>,
    #[serde(default, deserialize_with = "present")]
    before: Option<Twips>,
    #[serde(default, deserialize_with = "present")]
    after: Option<Twips>,
    #[serde(default, deserialize_with = "present")]
    line: Option<LineSpacing>,
    #[serde(default, deserialize_with = "present")]
    indent: Option<Twips>,
    #[serde(default, deserialize_with = "present")]
    keep_next: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    fill: Option<ColorRef>,
    #[serde(default, deserialize_with = "present")]
    border: Option<RawBorder<BorderSpacing>>,
    #[serde(default, deserialize_with = "present")]
    border_top: Option<RawBorder<BorderSpace>>,
    #[serde(default, deserialize_with = "present")]
    border_bottom: Option<RawBorder<BorderSpace>>,
    #[serde(default, deserialize_with = "present")]
    hyphenate: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    char_spacing: Option<SignedTwips>,
    #[serde(default, deserialize_with = "present")]
    kern: Option<HalfPoints>,
    #[serde(default, deserialize_with = "present")]
    proofing: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    snap_to_grid: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    contextual_spacing: Option<bool>,
}

/// A colour name from `colors` or a hex colour.
struct ColorRef(String);

/// Border distance: one value for all sides, or two values (top and bottom,
/// then left and right).
#[derive(Default)]
struct BorderSpacing {
    top_bottom: BorderSpace,
    left_right: BorderSpace,
}

/// A text value that is not empty.
struct Text(String);

impl RawParams {
    fn resolve(self) -> Result<DocxParams> {
        let colors = &self.colors;
        for name in colors.keys() {
            if Rgb::from_hex(name).is_some() {
                bail!("colors.{name}: colour name is also a hex value");
            }
        }
        let (width, height) = self.page.size.dimensions();
        let margins = &self.page.margins;
        if margins.left.0 + margins.right.0 >= width {
            bail!("page.margins: left and right margins must be less than the page width ({width} tw)");
        }
        if margins.top.0 + margins.bottom.0 >= height {
            bail!("page.margins: top and bottom margins must be less than the page height ({height} tw)");
        }
        for (key, height) in [
            ("title_height", self.logo.title_height),
            ("header_height", self.logo.header_height),
        ] {
            if height.0 == 0 {
                bail!("logo.{key}: the logo height must be more than 0");
            }
        }
        if !self.fonts.contains_key("body") {
            bail!("fonts: missing font role 'body'");
        }
        for name in self.styles.keys() {
            check_style_id(name)?;
        }
        check_style_names(&self.styles)?;
        check_based_on(&self.styles)?;

        let table = &self.table;
        let border = Border {
            width: table.border.width,
            color: lookup_color(colors, &table.border.color)
                .map_err(|m| anyhow!("table.border.color: {m}"))?,
            space: table.border.space,
        };
        let header_fill = lookup_color(colors, &table.header.fill)
            .map_err(|m| anyhow!("table.header.fill: {m}"))?;

        let mut styles = BTreeMap::new();
        for (name, raw) in self.styles {
            let style = raw.resolve(&name, &self.fonts, colors)?;
            styles.insert(name, style);
        }
        check_kinds(&styles)?;
        match styles.get("Normal") {
            None => bail!("styles: missing style 'Normal'"),
            Some(normal) if normal.based_on.is_some() => {
                bail!("styles.Normal.based_on: Normal cannot have a base style")
            }
            Some(_) => {}
        }

        Ok(DocxParams {
            page: self.page,
            logo: self.logo,
            fonts: self.fonts,
            table: TableParams {
                cell_margins: self.table.cell_margins,
                border,
                header: TableHeader {
                    bold: self.table.header.bold,
                    fill: header_fill,
                },
            },
            lists: self.lists,
            styles,
        })
    }
}

impl RawStyle {
    fn resolve(
        self,
        name: &str,
        fonts: &BTreeMap<String, String>,
        colors: &Colors,
    ) -> Result<Style> {
        let color = |field: &str, value: Option<ColorRef>| {
            value
                .map(|c| lookup_color(colors, &c))
                .transpose()
                .map_err(|m| anyhow!("styles.{name}.{field}: {m}"))
        };
        // Builds the border of one side.
        let side = |field: &str, width, color: &ColorRef, space| -> Result<Border> {
            let color = lookup_color(colors, color)
                .map_err(|m| anyhow!("styles.{name}.{field}.color: {m}"))?;
            Ok(Border {
                width,
                color,
                space,
            })
        };

        let kind = StyleKind::of(name);
        if kind == StyleKind::Character {
            let paragraph = [
                ("align", self.align.is_some()),
                ("before", self.before.is_some()),
                ("after", self.after.is_some()),
                ("line", self.line.is_some()),
                ("indent", self.indent.is_some()),
                ("keep_next", self.keep_next.is_some()),
                ("border", self.border.is_some()),
                ("border_top", self.border_top.is_some()),
                ("border_bottom", self.border_bottom.is_some()),
                ("hyphenate", self.hyphenate.is_some()),
                ("snap_to_grid", self.snap_to_grid.is_some()),
                ("contextual_spacing", self.contextual_spacing.is_some()),
            ];
            if let Some((field, _)) = paragraph.iter().find(|(_, set)| *set) {
                bail!("styles.{name}.{field}: paragraph property in a character style");
            }
        }

        let font = self
            .font
            .map(|role| {
                fonts
                    .get(&role)
                    .cloned()
                    .ok_or_else(|| anyhow!("styles.{name}.font: unknown font role '{role}'"))
            })
            .transpose()?;

        let mut borders = Borders::default();
        if let Some(raw) = &self.border {
            let top = side("border", raw.width, &raw.color, raw.space.top_bottom)?;
            let left = Border {
                space: raw.space.left_right,
                ..top
            };
            borders = Borders {
                top: Some(top),
                bottom: Some(top),
                left: Some(left),
                right: Some(left),
            };
        }
        if let Some(raw) = &self.border_top {
            borders.top = Some(side("border_top", raw.width, &raw.color, raw.space)?);
        }
        if let Some(raw) = &self.border_bottom {
            borders.bottom = Some(side("border_bottom", raw.width, &raw.color, raw.space)?);
        }

        Ok(Style {
            kind,
            based_on: self.based_on,
            font,
            size: self.size,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            color: color("color", self.color)?,
            align: self.align,
            before: self.before,
            after: self.after,
            line: self.line,
            indent: self.indent,
            keep_next: self.keep_next,
            fill: color("fill", self.fill)?,
            borders,
            hyphenate: self.hyphenate,
            char_spacing: self.char_spacing,
            kern: self.kern,
            proofing: self.proofing,
            snap_to_grid: self.snap_to_grid,
            contextual_spacing: self.contextual_spacing,
        })
    }
}

/// Finds a colour name in `colors`. If the name is not there, parses it as a
/// hex colour.
fn lookup_color(colors: &Colors, value: &ColorRef) -> Result<Rgb, String> {
    if let Some(&rgb) = colors.get(&value.0) {
        return Ok(rgb);
    }
    Rgb::from_hex(&value.0).ok_or_else(|| format!("unknown colour '{}'", value.0))
}

/// Makes sure that a style ID is ASCII letters and digits that start with a
/// letter, and that the ID is not reserved.
fn check_style_id(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    let valid = bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric());
    if !valid {
        bail!("styles.'{name}': invalid style ID; use ASCII letters and digits that start with a letter");
    }
    if RESERVED_STYLES.contains(&name) {
        bail!("styles.{name}: reserved style ID");
    }
    Ok(())
}

/// Makes sure that no two styles have the same display name (`w:name`).
/// Case is not significant. The names of all built-in styles are taken,
/// also when the theme does not define the style.
fn check_style_names(styles: &BTreeMap<String, RawStyle>) -> Result<()> {
    let mut owners: HashMap<String, &str> = stylesheet::builtin_styles()
        .map(|(id, name)| (name.to_ascii_lowercase(), id))
        .collect();
    for id in styles.keys() {
        let name = stylesheet::style_name(id);
        match owners.entry(name.to_ascii_lowercase()) {
            hash_map::Entry::Occupied(owner) if *owner.get() != id => {
                bail!(
                    "styles.{id}: name '{name}' is also the name of style '{}'",
                    owner.get()
                )
            }
            hash_map::Entry::Occupied(_) => {}
            hash_map::Entry::Vacant(slot) => {
                slot.insert(id);
            }
        }
    }
    Ok(())
}

/// Makes sure that each style has the same type as its base style.
fn check_kinds(styles: &BTreeMap<String, Style>) -> Result<()> {
    for (name, style) in styles {
        let Some(parent) = style.based_on.as_deref() else {
            continue;
        };
        let parent_kind = styles[parent].kind;
        if parent_kind != style.kind {
            bail!(
                "styles.{name}.based_on: '{parent}' is a {parent_kind} style; a {} style must be based on a {} style",
                style.kind,
                style.kind
            );
        }
    }
    Ok(())
}

/// Makes sure that each `based_on` names a style in `styles` and that no
/// chain of `based_on` links makes a cycle. Each style is on one chain only.
fn check_based_on(styles: &BTreeMap<String, RawStyle>) -> Result<()> {
    // Styles whose chain ends without a cycle.
    let mut done = HashSet::new();
    // The current chain, and the position of each style in it.
    let mut chain = Vec::new();
    let mut position = HashMap::new();
    for name in styles.keys() {
        let mut current = name.as_str();
        while !done.contains(current) {
            if let Some(&start) = position.get(current) {
                bail!(
                    "styles.{current}.based_on: cycle {} -> {current}",
                    chain[start..].join(" -> ")
                );
            }
            position.insert(current, chain.len());
            chain.push(current);
            let Some(parent) = styles[current].based_on.as_deref() else {
                break;
            };
            if !styles.contains_key(parent) {
                bail!("styles.{current}.based_on: unknown style '{parent}'");
            }
            current = parent;
        }
        done.extend(chain.drain(..));
        position.clear();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scalar parsing
// ---------------------------------------------------------------------------

/// Splits a value into its number and the text after the number. The number
/// is `-?[0-9]+(\.[0-9]+)?`. Gives `None` if the value does not start with
/// such a number.
fn split_number(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let digits = |from: usize| {
        from + bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let start = usize::from(bytes.first() == Some(&b'-'));
    let mut end = digits(start);
    if end == start {
        return None;
    }
    if bytes.get(end) == Some(&b'.') {
        let fraction_end = digits(end + 1);
        if fraction_end == end + 1 {
            return None;
        }
        end = fraction_end;
    }
    Some(s.split_at(end))
}

/// A number times a unit factor, as the exact fraction `n / q` with `q > 0`.
#[derive(Clone, Copy)]
struct Scaled {
    n: i128,
    q: i128,
}

impl Scaled {
    /// Gives `number * num / den`. `number` is a number from [`split_number`].
    fn new(number: &str, num: i128, den: i128) -> Result<Self, String> {
        let (int, fraction) = number.split_once('.').unwrap_or((number, ""));
        let (sign, int) = match int.strip_prefix('-') {
            Some(int) => (-1, int),
            None => (1, int),
        };
        if int.len() + fraction.len() > MAX_DIGITS {
            return Err(format!(
                "number '{number}' is too large; use at most {MAX_DIGITS} digits"
            ));
        }
        let digits = int
            .bytes()
            .chain(fraction.bytes())
            .fold(0, |v, b| v * 10 + i128::from(b - b'0'));
        Ok(Scaled {
            n: sign * digits * num,
            q: 10i128.pow(fraction.len() as u32) * den,
        })
    }

    /// Gives the value rounded half away from zero.
    fn round(self) -> i128 {
        (2 * self.n.abs() + self.q) / (2 * self.q) * self.n.signum()
    }

    /// Gives the value if it is a whole number.
    fn whole(self) -> Option<i128> {
        (self.n % self.q == 0).then_some(self.n / self.q)
    }

    /// Tells if the value is from `min` to `max`.
    fn within(self, min: i128, max: i128) -> bool {
        (min * self.q..=max * self.q).contains(&self.n)
    }
}

/// Parses a length with a unit and gives it in twips.
fn length(s: &str) -> Result<Scaled, String> {
    if s.is_empty() {
        return Err("missing value; use a number and a unit such as 12pt".to_owned());
    }
    let Some((number, unit)) = split_number(s) else {
        return Err(format!(
            "invalid number in '{s}'; use a number and a unit such as 12pt"
        ));
    };
    let (num, den) = match unit {
        "pt" => (20, 1),
        "cm" => (567, 1),
        "mm" => (567, 10),
        "in" => (1440, 1),
        "tw" => (1, 1),
        "" => return Err(format!("missing unit in '{s}'; use pt, cm, mm, in or tw")),
        other => {
            return Err(format!(
                "unknown unit '{other}' in '{s}'; use pt, cm, mm, in or tw"
            ))
        }
    };
    let twips = Scaled::new(number, num, den)?;
    if unit == "tw" && twips.whole().is_none() {
        return Err(format!("'{s}' must be a whole number of twips"));
    }
    if !twips.within(-MAX_TWIPS, MAX_TWIPS) {
        return Err(format!("length '{s}' is too large; the maximum is 22in"));
    }
    Ok(twips)
}

/// Parses a value that must be in `pt` and gives it times `factor`.
fn points(s: &str, factor: i128) -> Result<Scaled, String> {
    match split_number(s) {
        Some((number, "pt")) => Scaled::new(number, factor, 1),
        _ => Err(format!(
            "invalid value '{s}'; use a number in pt, such as 11pt"
        )),
    }
}

impl Twips {
    fn parse(s: &str) -> Result<Self, String> {
        let twips = length(s)?;
        if twips.n < 0 {
            return Err(format!("negative length '{s}'"));
        }
        Ok(Twips(twips.round() as u32))
    }
}

impl SignedTwips {
    fn parse(s: &str) -> Result<Self, String> {
        Ok(SignedTwips(length(s)?.round() as i32))
    }
}

impl HalfPoints {
    fn parse(s: &str) -> Result<Self, String> {
        let half_points = points(s, 2)?;
        if !half_points.within(1, 3276) {
            return Err(format!("'{s}' must be from 0.5pt to 1638pt"));
        }
        let Some(half_points) = half_points.whole() else {
            return Err(format!("'{s}' must be a multiple of 0.5pt"));
        };
        Ok(HalfPoints(half_points as u32))
    }
}

impl EighthPoints {
    fn parse(s: &str) -> Result<Self, String> {
        let eighths = points(s, 8)?;
        if !eighths.within(2, 96) {
            return Err(format!("'{s}' must be from 0.25pt to 12pt"));
        }
        let Some(eighths) = eighths.whole() else {
            return Err(format!("'{s}' must be a multiple of 0.125pt"));
        };
        Ok(EighthPoints(eighths as u8))
    }
}

impl BorderSpace {
    fn parse(s: &str) -> Result<Self, String> {
        match points(s, 1)?.whole() {
            Some(value @ 0..=31) => Ok(BorderSpace(value as u8)),
            _ => Err(format!(
                "'{s}' must be a whole number of points from 0pt to 31pt"
            )),
        }
    }
}

impl BorderSpacing {
    /// Two values have one space between them.
    fn parse(s: &str) -> Result<Self, String> {
        let mut parts = s.split(' ');
        let (Some(first), second, None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!(
                "'{s}' must have one or two values, such as 1pt 4pt"
            ));
        };
        let top_bottom = BorderSpace::parse(first)?;
        let left_right = match second {
            Some(second) => BorderSpace::parse(second)?,
            None => top_bottom,
        };
        Ok(BorderSpacing {
            top_bottom,
            left_right,
        })
    }
}

impl LineSpacing {
    /// A number without a unit is a multiple of the single line height. A
    /// length with a unit is an exact line height that is more than 0.
    fn parse(s: &str) -> Result<Self, String> {
        let Some((number, "")) = split_number(s) else {
            return match Twips::parse(s)? {
                Twips(0) => Err(format!("exact line spacing '{s}' must be more than 0")),
                twips => Ok(LineSpacing::Exact(twips)),
            };
        };
        let line = Scaled::new(number, 240, 1)?;
        if line.within(120, 1200) {
            Ok(LineSpacing::Auto(line.round() as u32))
        } else if line.n > 1200 * line.q {
            Err(format!(
                "line multiple '{s}' must be from 0.5 to 5; use {s}pt for exact spacing"
            ))
        } else {
            Err(format!("line multiple '{s}' must be from 0.5 to 5"))
        }
    }
}

impl Rgb {
    /// Parses `RRGGBB` or `#RRGGBB` in any case.
    fn parse(s: &str) -> Result<Self, String> {
        if s.is_empty() {
            return Err(missing_color());
        }
        Rgb::from_hex(s).ok_or_else(|| format!("invalid colour '{s}'; use RRGGBB or \"#RRGGBB\""))
    }

    /// Gives the colour of `RRGGBB` or `#RRGGBB` in any case.
    fn from_hex(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let [_, r, g, b] = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
        Some(Rgb([r, g, b]))
    }
}

impl ColorRef {
    fn parse(s: &str) -> Result<Self, String> {
        if s.is_empty() {
            return Err(missing_color());
        }
        Ok(ColorRef(s.to_owned()))
    }
}

impl Text {
    /// A YAML null (`~`, `null`, `Null` or `NULL`) is an empty value. A
    /// control character, U+FFFE and U+FFFF are not permitted, because XML
    /// cannot contain most control characters and cannot contain U+FFFE or
    /// U+FFFF.
    fn parse(s: &str) -> Result<Self, String> {
        if s.trim().is_empty() || matches!(s, "~" | "null" | "Null" | "NULL") {
            return Err("empty value".to_owned());
        }
        if let Some(c) = s
            .chars()
            .find(|&c| c.is_control() || matches!(c, '\u{FFFE}' | '\u{FFFF}'))
        {
            return Err(format!("invalid character U+{:04X} in value", u32::from(c)));
        }
        Ok(Text(s.to_owned()))
    }
}

/// An unquoted YAML value that starts with `#` is a comment. Thus the
/// colour arrives as an empty value.
fn missing_color() -> String {
    "missing colour; a value that starts with '#' must be in quotes, such as \"#EE721C\"".to_owned()
}

// ---------------------------------------------------------------------------
// serde glue
// ---------------------------------------------------------------------------

/// Reads a YAML scalar as text and parses it with `parse`.
struct ScalarVisitor<T> {
    expecting: &'static str,
    parse: fn(&str) -> Result<T, String>,
}

impl<T> Visitor<'_> for ScalarVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        (self.parse)(v).map_err(E::custom)
    }
}

macro_rules! scalar_deserialize {
    ($($ty:ty => $expecting:literal),* $(,)?) => {$(
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                d.deserialize_str(ScalarVisitor {
                    expecting: $expecting,
                    parse: <$ty>::parse,
                })
            }
        }
    )*};
}

scalar_deserialize! {
    Twips => "a length such as 12pt",
    SignedTwips => "a length such as -0.5pt",
    HalfPoints => "a size in pt such as 11pt",
    EighthPoints => "a border width in pt such as 0.5pt",
    BorderSpace => "a border space in pt such as 1pt",
    BorderSpacing => "one or two border spaces in pt such as 1pt 4pt",
    LineSpacing => "a line multiple such as 1.15 or a length such as 14pt",
    Rgb => "a hex colour such as EE721C",
    ColorRef => "a colour name or a hex colour",
    Text => "a text value",
}

/// Reads an optional field that is present in YAML. An empty value goes to
/// the value parser and does not become `None`.
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

/// Reads a map with string keys. A duplicate key is an error.
struct UniqueMap<V> {
    what: &'static str,
    marker: PhantomData<V>,
}

impl<'de, V: Deserialize<'de>> Visitor<'de> for UniqueMap<V> {
    type Value = BTreeMap<String, V>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a map of {}s", self.what)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            match out.entry(key) {
                Entry::Occupied(entry) => {
                    return Err(de::Error::custom(format!(
                        "duplicate {} '{}'",
                        self.what,
                        entry.key()
                    )))
                }
                Entry::Vacant(entry) => {
                    entry.insert(map.next_value()?);
                }
            }
        }
        Ok(out)
    }
}

fn unique_map<'de, D, V>(d: D, what: &'static str) -> Result<BTreeMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    d.deserialize_map(UniqueMap {
        what,
        marker: PhantomData,
    })
}

fn styles_map<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, RawStyle>, D::Error> {
    unique_map(d, "style")
}

fn colors_map<'de, D: Deserializer<'de>>(d: D) -> Result<Colors, D::Error> {
    unique_map(d, "colour")
}

fn fonts_map<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    let fonts: BTreeMap<String, Text> = unique_map(d, "font role")?;
    Ok(fonts
        .into_iter()
        .map(|(role, name)| (role, name.0))
        .collect())
}

fn bullets<'de, D: Deserializer<'de>>(d: D) -> Result<[String; 6], D::Error> {
    <[Text; 6]>::deserialize(d).map(|bullets| bullets.map(|bullet| bullet.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
page:
  size: a4
  margins: {top: 2cm, bottom: 2cm, left: 2cm, right: 1cm, header: 1.5cm, footer: 1.75cm}
logo: {title_height: 0.92cm, header_height: 10.5pt}
fonts: {body: PT Sans, heading: PT Sans, mono: PT Mono}
colors: {accent: EE721C, code_border: D9D9D9, code_fill: F2F2F2}
table:
  cell_margins: {top: 0tw, bottom: 0tw, left: 10tw, right: 10tw}
  border: {width: 0.25pt, color: "000000", space: 0pt}
  header: {bold: true, fill: code_fill}
lists:
  indent: 720tw
  hanging: 360tw
  bullets: ["•", "◦", "▪", "•", "◦", "▪"]
  numbers: [decimal, lower_letter, lower_roman, decimal, lower_letter, lower_roman]
"#;

    /// Appends a `styles:` block to `base`.
    fn yaml(base: &str, styles: &str) -> String {
        format!("{base}styles:\n{styles}")
    }

    fn parse(styles: &str) -> DocxParams {
        DocxParams::from_yaml(&yaml(BASE, styles)).unwrap()
    }

    fn error(text: &str) -> String {
        format!("{:#}", DocxParams::from_yaml(text).unwrap_err())
    }

    fn style_error(styles: &str) -> String {
        error(&yaml(BASE, styles))
    }

    #[track_caller]
    fn assert_starts(err: &str, prefix: &str) {
        assert!(err.starts_with(prefix), "error: {err}\nexpected: {prefix}");
    }

    #[test]
    fn twips_units() {
        for (text, twips) in [
            ("1pt", 20),
            ("11.5pt", 230),
            ("1in", 1440),
            ("1cm", 567),
            ("2cm", 1134),
            ("1.5cm", 851),
            ("1.75cm", 992),
            ("10mm", 567),
            ("851tw", 851),
            ("0pt", 0),
            ("-0pt", 0),
            ("0.021875in", 32),
            ("1.009375in", 1454),
            ("0.05mm", 3),
            ("22in", 31680),
        ] {
            assert_eq!(Twips::parse(text), Ok(Twips(twips)), "{text}");
        }
    }

    #[test]
    fn twips_rejects() {
        for text in [
            "12",
            "0",
            "",
            "pt",
            "12px",
            "12 pt",
            "12PT",
            "1.5tw",
            "-1pt",
            "1e3pt",
            "99999in",
            "inf",
            "nan",
            ".5pt",
            "-0.01pt",
            "22.0001in",
            "1584.01pt",
        ] {
            assert!(Twips::parse(text).is_err(), "{text}");
        }
        assert_eq!(
            Twips::parse("12").unwrap_err(),
            "missing unit in '12'; use pt, cm, mm, in or tw"
        );
        assert_eq!(
            Twips::parse("12px").unwrap_err(),
            "unknown unit 'px' in '12px'; use pt, cm, mm, in or tw"
        );
        let long = format!("1{}pt", "0".repeat(40));
        assert_starts(&Twips::parse(&long).unwrap_err(), "number '1000");
        assert!(Twips::parse(&format!("0.{}1pt", "0".repeat(40))).is_err());
    }

    #[test]
    fn signed_twips() {
        assert_eq!(SignedTwips::parse("-0.5pt"), Ok(SignedTwips(-10)));
        assert_eq!(SignedTwips::parse("-0.021875in"), Ok(SignedTwips(-32)));
        assert_eq!(SignedTwips::parse("-0pt"), Ok(SignedTwips(0)));
        assert!(SignedTwips::parse("-99999in").is_err());
    }

    #[test]
    fn half_points() {
        assert_eq!(HalfPoints::parse("11pt"), Ok(HalfPoints(22)));
        assert_eq!(HalfPoints::parse("10.5pt"), Ok(HalfPoints(21)));
        for text in [
            "10.3pt",
            "11",
            "4mm",
            "0pt",
            "0.3pt",
            "1638.2pt",
            "11.00000000000000000001pt",
        ] {
            assert!(HalfPoints::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn eighth_points() {
        assert_eq!(EighthPoints::parse("0.5pt"), Ok(EighthPoints(4)));
        assert_eq!(EighthPoints::parse("0.25pt"), Ok(EighthPoints(2)));
        for text in [
            "0.3pt",
            "0.125pt",
            "13pt",
            "0.2499999999999999999pt",
            "12.0000000000000000001pt",
        ] {
            assert!(EighthPoints::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn border_spacing() {
        let spacing = |text| BorderSpacing::parse(text).map(|s| (s.top_bottom.0, s.left_right.0));
        assert_eq!(spacing("1pt"), Ok((1, 1)));
        assert_eq!(spacing("1pt 4pt"), Ok((1, 4)));
        assert_eq!(spacing("-0pt"), Ok((0, 0)));
        for text in [
            "1.5pt",
            "32pt",
            "1pt 2pt 3pt",
            "",
            "1pt  4pt",
            "1pt\t4pt",
            "1pt\u{a0}4pt",
            "31.0000000000000000001pt",
        ] {
            assert!(BorderSpacing::parse(text).is_err(), "{text}");
        }
        assert_eq!(BorderSpace::parse("31pt"), Ok(BorderSpace(31)));
        assert!(BorderSpace::parse("1pt 4pt").is_err());
    }

    #[test]
    fn line_spacing() {
        assert_eq!(LineSpacing::parse("1"), Ok(LineSpacing::Auto(240)));
        assert_eq!(LineSpacing::parse("1.15"), Ok(LineSpacing::Auto(276)));
        assert_eq!(
            LineSpacing::parse("14pt"),
            Ok(LineSpacing::Exact(Twips(280)))
        );
        assert_eq!(LineSpacing::parse("1.05625"), Ok(LineSpacing::Auto(254)));
        assert_eq!(LineSpacing::parse("0.5"), Ok(LineSpacing::Auto(120)));
        assert_eq!(LineSpacing::parse("5"), Ok(LineSpacing::Auto(1200)));
        for text in [
            "0",
            "-0",
            "0pt",
            "0tw",
            "0.4999999999999999999",
            "5.0000000000000000001",
        ] {
            assert!(LineSpacing::parse(text).is_err(), "{text}");
        }
        let err = LineSpacing::parse("12").unwrap_err();
        assert!(err.contains("use 12pt for exact spacing"), "{err}");
    }

    #[test]
    fn colours() {
        assert_eq!(Rgb::parse("ee721c"), Ok(Rgb([0xEE, 0x72, 0x1C])));
        assert_eq!(Rgb::parse("#EE721C").unwrap().to_string(), "EE721C");
        assert!(Rgb::parse("EE72").is_err());
        assert!(Rgb::parse("GG0000").is_err());

        let quoted: Rgb = serde_yaml::from_str(r##""#ee721c""##).unwrap();
        assert_eq!(quoted.to_string(), "EE721C");
        let plain: Rgb = serde_yaml::from_str("000000").unwrap();
        assert_eq!(plain, Rgb([0, 0, 0]));
    }

    #[test]
    fn unquoted_hash_colour_is_an_error() {
        let err = style_error("  Heading1:\n    color: #EE721C\n");
        assert_starts(
            &err,
            "styles.Heading1.color: missing colour; a value that starts with '#' must be in quotes",
        );

        let err = error(&yaml(
            &BASE.replace(
                "colors: {accent: EE721C, code_border: D9D9D9, code_fill: F2F2F2}",
                "colors:\n  accent: #EE721C",
            ),
            "  Normal: {}\n",
        ));
        assert_starts(&err, "colors.accent: missing colour");
    }

    #[test]
    fn resolves_references() {
        let params = parse(
            r##"
  Normal: {}
  Heading1:
    font: heading
    color: accent
    fill: "#f2f2f2"
    border_bottom: {width: 0.5pt, color: accent, space: 1pt}
  Code:
    font: mono
    color: 333333
    border: {width: 0.5pt, color: code_border, space: 1pt 4pt}
    border_top: {width: 1pt, color: accent, space: 2pt}
    border_bottom: {width: 1pt, color: "#000000"}
"##,
        );
        let h1 = &params.styles["Heading1"];
        assert_eq!(h1.font.as_deref(), Some("PT Sans"));
        assert_eq!(h1.color, Some(Rgb([0xEE, 0x72, 0x1C])));
        assert_eq!(h1.fill, Some(Rgb([0xF2, 0xF2, 0xF2])));
        assert_eq!(h1.borders.top, None);
        assert_eq!(h1.borders.left, None);
        assert_eq!(
            h1.borders.bottom,
            Some(Border {
                width: EighthPoints(4),
                color: Rgb([0xEE, 0x72, 0x1C]),
                space: BorderSpace(1),
            })
        );

        let code = &params.styles["Code"];
        assert_eq!(code.font.as_deref(), Some("PT Mono"));
        assert_eq!(code.color.unwrap().to_string(), "333333");
        let side = |b: Option<Border>| b.map(|b| (b.width.0, b.color.to_string(), b.space.0));
        assert_eq!(side(code.borders.top), Some((8, "EE721C".into(), 2)));
        assert_eq!(side(code.borders.bottom), Some((8, "000000".into(), 0)));
        assert_eq!(side(code.borders.left), Some((4, "D9D9D9".into(), 4)));
        assert_eq!(side(code.borders.right), Some((4, "D9D9D9".into(), 4)));

        assert_eq!(params.table.border.color, Rgb([0, 0, 0]));
        assert_eq!(params.table.border.width, EighthPoints(2));
        assert_eq!(params.table.border.space, BorderSpace(0));
        assert!(params.table.header.bold);
        assert_eq!(params.table.header.fill, Rgb([0xF2, 0xF2, 0xF2]));
        assert_eq!(params.fonts["body"], "PT Sans");
    }

    #[test]
    fn resolution_errors() {
        assert_starts(
            &style_error("  Heading1: {color: acent}\n"),
            "styles.Heading1.color: unknown colour 'acent'",
        );
        assert_starts(
            &style_error("  Heading1: {fill: nope}\n"),
            "styles.Heading1.fill: unknown colour 'nope'",
        );
        assert_starts(
            &style_error("  Heading1: {font: headng}\n"),
            "styles.Heading1.font: unknown font role 'headng'",
        );
        assert_starts(
            &style_error("  CodeBlock:\n    border: {width: 0.5pt, color: grey}\n"),
            "styles.CodeBlock.border.color: unknown colour 'grey'",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replace(r#"color: "000000""#, "color: black"),
                "  Normal: {}\n",
            )),
            "table.border.color: unknown colour 'black'",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replace("fill: code_fill", "fill: code_fil"),
                "  Normal: {}\n",
            )),
            "table.header.fill: unknown colour 'code_fil'",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replace("code_fill: F2F2F2", "beaded: F2F2F2"),
                "  Normal: {}\n",
            )),
            "colors.beaded: colour name is also a hex value",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replace("body: PT Sans, ", ""),
                "  Normal: {}\n",
            )),
            "fonts: missing font role 'body'",
        );
        assert_starts(
            &style_error("  Heading1: {based_on: Normall}\n"),
            "styles.Heading1.based_on: unknown style 'Normall'",
        );
        assert_starts(
            &style_error("  A: {based_on: B}\n  B: {based_on: A}\n"),
            "styles.A.based_on: cycle A -> B -> A",
        );
        assert_starts(
            &style_error("  A: {based_on: B}\n  B: {based_on: C}\n  C: {based_on: B}\n"),
            "styles.B.based_on: cycle B -> C -> B",
        );
        assert_starts(
            &style_error("  A: {based_on: A}\n"),
            "styles.A.based_on: cycle A -> A",
        );
    }

    #[test]
    fn style_checks() {
        assert_starts(
            &style_error("  Heading1: {size: 28pt}\n"),
            "styles: missing style 'Normal'",
        );
        assert_starts(
            &style_error("  Normal: {based_on: Base}\n  Base: {}\n"),
            "styles.Normal.based_on: Normal cannot have a base style",
        );
        for id in ["DataTable", "TableNormal", "DefaultParagraphFont", "NoList"] {
            assert_starts(
                &style_error(&format!("  Normal: {{}}\n  {id}: {{}}\n")),
                &format!("styles.{id}: reserved style ID"),
            );
        }
        for id in [
            "1Heading",
            "Heading_1",
            "\"Heading 1\"",
            "\"\"",
            "Überschrift",
        ] {
            let err = style_error(&format!("  Normal: {{}}\n  {id}: {{}}\n"));
            assert!(
                err.starts_with("styles.'") && err.contains("': invalid style ID;"),
                "{id}: {err}"
            );
        }
        assert_starts(
            &style_error("  Normal: {}\n  A.B: {}\n"),
            "styles.'A.B': invalid style ID;",
        );
        assert_starts(
            &style_error("  Normal: {}\n  header: {}\n"),
            "styles.header: name 'header' is also the name of style 'Header'",
        );
        assert_starts(
            &style_error("  Normal: {}\n  NormalTable: {}\n"),
            "styles.NormalTable: name 'Normal Table' is also the name of style 'TableNormal'",
        );
        assert_starts(
            &style_error("  Normal: {}\n  CodeBlock: {}\n  Codeblock: {}\n  codeBlock: {}\n"),
            "styles.codeBlock: name 'code Block' is also the name of style 'CodeBlock'",
        );
        assert_starts(
            &style_error("  Normal: {}\n  InlineCode: {font: mono, align: left}\n"),
            "styles.InlineCode.align: paragraph property in a character style",
        );
        assert_starts(
            &style_error(
                "  Normal: {}\n  Hyperlink:\n    border_bottom: {width: 1pt, color: accent}\n",
            ),
            "styles.Hyperlink.border_bottom: paragraph property in a character style",
        );
        assert_starts(
            &style_error("  Normal: {}\n  InlineCode: {based_on: Normal}\n"),
            "styles.InlineCode.based_on: 'Normal' is a paragraph style; a character style must be based on a character style",
        );
        assert_starts(
            &style_error("  Normal: {}\n  InlineCode: {}\n  Code: {based_on: InlineCode}\n"),
            "styles.Code.based_on: 'InlineCode' is a character style; a paragraph style must be based on a paragraph style",
        );

        let params = parse(
            "  Normal: {}\n  InlineCode: {fill: code_fill}\n  Hyperlink: {based_on: InlineCode}\n  Strong: {bold: true}\n  Emphasis: {}\n  Header: {}\n  CodeBlock: {}\n  Codeblock: {}\n",
        );
        assert_eq!(params.styles["Normal"].kind, StyleKind::Paragraph);
        for id in ["InlineCode", "Hyperlink", "Strong", "Emphasis"] {
            assert_eq!(params.styles[id].kind, StyleKind::Character, "{id}");
        }
    }

    #[test]
    fn yaml_errors_have_paths() {
        assert_starts(
            &style_error("  Heading1: {colour: accent}\n"),
            "styles.Heading1: unknown field `colour`",
        );
        assert_starts(
            &style_error("  Heading1:\n    before: 12\n"),
            "styles.Heading1.before: missing unit in '12'; use pt, cm, mm, in or tw",
        );
        assert_starts(
            &style_error("  Heading1:\n    size: 11\n"),
            "styles.Heading1.size: invalid value '11'",
        );
        assert_starts(
            &style_error(
                "  Heading1:\n    border: {width: 0.5pt, color: accent, space: 1pt 2pt 3pt}\n",
            ),
            "styles.Heading1.border.space: '1pt 2pt 3pt' must have one or two values",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replace("size: a4", "size: a5"),
                "  Normal: {}\n",
            )),
            "page.size: unknown variant `a5`",
        );
        assert_starts(
            &style_error("  Normal: {align: justified}\n"),
            "styles.Normal.align: unknown variant `justified`",
        );
        assert_starts(
            &style_error("  Normal: {bold: yes}\n"),
            "styles.Normal.bold: invalid type",
        );
        assert_starts(&error(BASE), "missing field `styles`");
        assert_starts(
            &style_error("  CodeBlock: {bold: true}\n  CodeBlock: {italic: true}\n"),
            "styles: duplicate style 'CodeBlock'",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replace("code_fill: F2F2F2", "code_fill: F2F2F2, accent: 000000"),
                "  Normal: {}\n",
            )),
            "colors: duplicate colour 'accent'",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replace("mono: PT Mono", "mono: PT Mono, body: Arial"),
                "  Normal: {}\n",
            )),
            "fonts: duplicate font role 'body'",
        );
        assert_starts(
            &error(&yaml(
                &BASE.replacen("\"•\", \"◦\", ", "", 1),
                "  Normal: {}\n",
            )),
            "lists.bullets: invalid length 4",
        );
        assert_starts(
            &error(&yaml(&BASE.replace("\"▪\"]", "\"\"]"), "  Normal: {}\n")),
            "lists.bullets[5]: empty value",
        );
        for (name, code) in [
            (r#""A\x01B""#, "U+0001"),
            (r#""A\nB""#, "U+000A"),
            (r#""A\tB""#, "U+0009"),
        ] {
            assert_starts(
                &error(&yaml(
                    &BASE.replace("mono: PT Mono", &format!("mono: {name}")),
                    "  Normal: {}\n",
                )),
                &format!("fonts.mono: invalid character {code} in value"),
            );
        }
        assert_starts(
            &style_error("  H:\n    border_bottom: {width: 1pt, color: accent, space: 3pt 9pt}\n"),
            "styles.H.border_bottom.space: invalid value '3pt 9pt'",
        );
        assert_starts(
            &style_error("  Normal:\n    size: # 11pt\n"),
            "styles.Normal.size: invalid value ''",
        );
        assert_starts(
            &style_error("  Normal:\n    bold:\n"),
            "styles.Normal.bold: invalid type",
        );
        assert_starts(
            &style_error("  Hyperlink:\n    underline:\n"),
            "styles.Hyperlink.underline: invalid type",
        );
        assert_starts(
            &style_error("  Normal: {before: ~}\n"),
            "styles.Normal.before: invalid number in '~'",
        );
    }

    #[test]
    fn xml_noncharacter_in_text() {
        for (name, code) in [(r#""A\uFFFEB""#, "U+FFFE"), (r#""A\uFFFFB""#, "U+FFFF")] {
            assert_starts(
                &error(&yaml(
                    &BASE.replace("mono: PT Mono", &format!("mono: {name}")),
                    "  Normal: {}\n",
                )),
                &format!("fonts.mono: invalid character {code} in value"),
            );
        }
    }

    #[test]
    fn null_text() {
        for text in ["~", "null", "Null", "NULL", " "] {
            assert!(Text::parse(text).is_err(), "{text}");
        }
        for null in ["null", "~"] {
            assert_starts(
                &error(&yaml(
                    &BASE.replace("body: PT Sans", &format!("body: {null}")),
                    "  Normal: {}\n",
                )),
                "fonts.body: empty value",
            );
        }
    }

    #[test]
    fn table_fields_are_required() {
        for (from, to, message) in [
            (", space: 0pt", "", "table.border: missing field `space`"),
            (
                ", fill: code_fill",
                "",
                "table.header: missing field `fill`",
            ),
            (
                "{bold: true, fill: code_fill}",
                "\n    bold: true\n    fill: #F2F2F2",
                "table.header.fill: missing colour",
            ),
        ] {
            let text = yaml(&BASE.replace(from, to), "  Normal: {}\n");
            assert_starts(&error(&text), message);
        }
    }

    #[test]
    fn logo_heights() {
        let params = parse("  Normal: {}\n");
        assert_eq!(params.logo.title_height, Twips(522));
        assert_eq!(params.logo.header_height, Twips(210));

        let logo = "logo: {title_height: 0.92cm, header_height: 10.5pt}\n";
        for (to, message) in [
            ("", "missing field `logo`"),
            (
                "logo: {title_height: 0.92cm}\n",
                "logo: missing field `header_height`",
            ),
            (
                "logo: {title_height: 0.92cm, header_height: 10.5pt, width: 2cm}\n",
                "logo: unknown field `width`",
            ),
            (
                "logo: {title_height: 0cm, header_height: 10.5pt}\n",
                "logo.title_height: the logo height must be more than 0",
            ),
            (
                "logo: {title_height: 0.92cm, header_height: 0pt}\n",
                "logo.header_height: the logo height must be more than 0",
            ),
            (
                "logo: {title_height: 0.92, header_height: 10.5pt}\n",
                "logo.title_height: missing unit in '0.92'",
            ),
        ] {
            let text = yaml(&BASE.replace(logo, to), "  Normal: {}\n");
            assert_starts(&error(&text), message);
        }
    }

    #[test]
    fn colors_section_is_optional() {
        let text = yaml(
            &BASE
                .replace(
                    "colors: {accent: EE721C, code_border: D9D9D9, code_fill: F2F2F2}\n",
                    "",
                )
                .replace("fill: code_fill", "fill: F2F2F2"),
            "  Normal: {color: 333333}\n",
        );
        let params = DocxParams::from_yaml(&text).unwrap();
        assert_eq!(params.styles["Normal"].color, Some(Rgb([0x33, 0x33, 0x33])));
    }

    #[test]
    fn full_sample() {
        let params = parse(
            r#"
  Normal:
    font: body
    align: justify
    line: 1
  Heading1:
    based_on: Normal
    font: heading
    size: 28pt
    bold: true
    color: accent
    align: left
    before: 360tw
    after: 0pt
    keep_next: true
    border_bottom: {width: 0.5pt, color: accent, space: 1pt}
  Title:
    based_on: Normal
    align: right
    size: 28pt
    underline: false
    char_spacing: -0.5pt
    kern: 14pt
    after: 0pt
    border_bottom: {width: 0.5pt, color: accent, space: 1pt}
  CodeBlock:
    based_on: Normal
    font: mono
    size: 10pt
    fill: code_fill
    border: {width: 0.5pt, color: code_border, space: 1pt 4pt}
    after: 0pt
    align: left
    hyphenate: false
    proofing: false
  TOC2:
    based_on: Normal
    indent: 200tw
    after: 100tw
    snap_to_grid: false
  AdmonitionNoteBody:
    based_on: Normal
    fill: D4E9F9
    border: {width: 0.5pt, color: 6CB4EE, space: 1pt 4pt}
    contextual_spacing: true
  Hyperlink:
    color: accent
    underline: true
"#,
        );

        assert_eq!(params.page.size, PageSize::A4);
        assert_eq!(params.page.size.dimensions(), (11906, 16838));
        let m = &params.page.margins;
        assert_eq!(
            [m.top, m.bottom, m.left, m.right, m.header, m.footer].map(|t| t.0),
            [1134, 1134, 1134, 567, 851, 992]
        );
        let c = &params.table.cell_margins;
        assert_eq!(
            [c.top, c.bottom, c.left, c.right].map(|t| t.0),
            [0, 0, 10, 10]
        );
        assert_eq!(params.lists.indent, Twips(720));
        assert_eq!(params.lists.hanging, Twips(360));
        assert_eq!(params.lists.bullets[1], "◦");
        assert_eq!(params.lists.numbers[2], NumberFormat::LowerRoman);

        let normal = &params.styles["Normal"];
        assert_eq!(normal.align, Some(Align::Justify));
        assert_eq!(normal.line, Some(LineSpacing::Auto(240)));

        let h1 = &params.styles["Heading1"];
        assert_eq!(h1.based_on.as_deref(), Some("Normal"));
        assert_eq!(h1.size, Some(HalfPoints(56)));
        assert_eq!(h1.before, Some(Twips(360)));
        assert_eq!(h1.after, Some(Twips(0)));
        assert_eq!(h1.bold, Some(true));
        assert_eq!(h1.italic, None);
        assert_eq!(h1.underline, None);

        let title = &params.styles["Title"];
        assert_eq!(title.char_spacing, Some(SignedTwips(-10)));
        assert_eq!(title.kern, Some(HalfPoints(28)));
        assert_eq!(title.align, Some(Align::Right));
        assert_eq!(title.underline, Some(false));

        let code = &params.styles["CodeBlock"];
        assert_eq!(code.size, Some(HalfPoints(20)));
        assert_eq!(code.hyphenate, Some(false));
        assert_eq!(code.proofing, Some(false));

        let toc = &params.styles["TOC2"];
        assert_eq!(toc.indent, Some(Twips(200)));
        assert_eq!(toc.snap_to_grid, Some(false));

        let note = &params.styles["AdmonitionNoteBody"];
        assert_eq!(note.contextual_spacing, Some(true));
        assert_eq!(note.borders.left.unwrap().color.to_string(), "6CB4EE");

        assert_eq!(params.styles["Hyperlink"].underline, Some(true));
    }

    #[test]
    fn text_width() {
        let params = crate::theme::Theme::Embedded("blank")
            .docx_params()
            .unwrap();
        assert_eq!(params.page.text_width(), 10205);
    }

    #[test]
    fn margins_must_leave_text_area() {
        let margins = "margins: {top: 2cm, bottom: 2cm, left: 2cm, right: 1cm,";
        let wide = BASE.replace(
            margins,
            "margins: {top: 2cm, bottom: 2cm, left: 11906tw, right: 0tw,",
        );
        assert_starts(
            &error(&yaml(&wide, "  Normal: {}\n")),
            "page.margins: left and right margins must be less than the page width (11906 tw)",
        );
        let tall = BASE.replace(
            margins,
            "margins: {top: 8419tw, bottom: 8419tw, left: 2cm, right: 1cm,",
        );
        assert_starts(
            &error(&yaml(&tall, "  Normal: {}\n")),
            "page.margins: top and bottom margins must be less than the page height (16838 tw)",
        );
        let fits = BASE.replace(
            margins,
            "margins: {top: 8418tw, bottom: 8419tw, left: 11905tw, right: 0tw,",
        );
        assert_eq!(
            DocxParams::from_yaml(&yaml(&fits, "  Normal: {}\n"))
                .unwrap()
                .page
                .text_width(),
            1
        );
    }
}
