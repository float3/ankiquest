//! Share cards the server draws for wins, and the public page behind a win's link.
//!
//! Cards are built as SVG, rasterised with resvg using the embedded DejaVu Sans,
//! and composited with the companion art and the player's avatar.
use crate::app::App;
use crate::store::{Error, Store};
use crate::wins::{self, Win};
use crate::{access::authorized, avatars, i18n};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use chrono::Datelike;
use resvg::{tiny_skia, usvg};
use sha2::{Digest, Sha256};
use skrifa::MetadataProvider;
use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::{Arc, Mutex, OnceLock};

const REGULAR: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");
const FAMILY: &str = "DejaVu Sans";
const INK: &str = "#3C3C3C";
const MUTED: &str = "#8A8A8A";
const PUBLIC_IMAGE: &str = "public, max-age=3600";
/// Rendered PNGs kept in memory, keyed by everything that went into them.
const CACHED: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// 1080×1080, for sharing the image itself.
    Square,
    /// 1200×630, for link previews.
    Wide,
}

impl Shape {
    pub(crate) fn size(self) -> (u32, u32) {
        match self {
            Shape::Square => (1080, 1080),
            Shape::Wide => (1200, 630),
        }
    }
}

/// Everything a card shows, already translated.
pub(crate) struct Card {
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) chips: Vec<String>,
    pub(crate) display: String,
    pub(crate) date: String,
    pub(crate) host: String,
    /// The name of the embedded companion art, if the player has a companion.
    pub(crate) art: Option<&'static str>,
    /// The player's avatar as PNG.
    pub(crate) avatar: Option<Vec<u8>>,
    pub(crate) seed: u64,
}

/// The embedded art for `pose` of `companion`; Ankilope and unknown poses celebrate.
pub(crate) fn art(companion: &str, pose: &str) -> Option<&'static str> {
    match (companion, pose) {
        ("none", _) | (_, "none") => None,
        ("ankilope", _) => Some("ankilope-celebrate"),
        (_, "streak") => Some("aki-streak"),
        (_, "winner") => Some("aki-winner"),
        (_, "freeze") => Some("aki-freeze"),
        _ => Some("aki-celebrate"),
    }
}

/// The decoded art called `name`, decoded once.
fn art_pixmap(name: &str) -> Option<&'static tiny_skia::Pixmap> {
    const NAMES: [&str; 5] = [
        "aki-celebrate",
        "aki-streak",
        "aki-winner",
        "aki-freeze",
        "ankilope-celebrate",
    ];
    static DECODED: [OnceLock<Option<tiny_skia::Pixmap>>; 5] = [const { OnceLock::new() }; 5];
    let index = NAMES.iter().position(|known| *known == name)?;
    DECODED[index]
        .get_or_init(|| tiny_skia::Pixmap::decode_png(art_png(name)?).ok())
        .as_ref()
}

fn art_png(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "aki-celebrate" => include_bytes!("../assets/cards/aki-celebrate.png"),
        "aki-streak" => include_bytes!("../assets/cards/aki-streak.png"),
        "aki-winner" => include_bytes!("../assets/cards/aki-winner.png"),
        "aki-freeze" => include_bytes!("../assets/cards/aki-freeze.png"),
        "ankilope-celebrate" => include_bytes!("../assets/cards/ankilope-celebrate.png"),
        _ => return None,
    })
}

struct Palette {
    fill: &'static str,
    light: &'static str,
    dark: &'static str,
    deep: &'static str,
    /// Text on `fill`.
    on: &'static str,
}

fn palette(kind: &str) -> Palette {
    match kind {
        "daily_goal" | "streak_secured" | "quest" => Palette {
            fill: "#58CC02",
            light: "#89E219",
            dark: "#58A700",
            deep: "#3B7A00",
            on: "#FFFFFF",
        },
        "streak" => Palette {
            fill: "#FF9600",
            light: "#FFB84D",
            dark: "#D97E00",
            deep: "#A35A00",
            on: "#FFFFFF",
        },
        "level" | "achievement" => Palette {
            fill: "#CE82FF",
            light: "#E3B5FF",
            dark: "#A560E8",
            deep: "#7238A8",
            on: "#FFFFFF",
        },
        "place" | "record" => Palette {
            fill: "#FFC800",
            light: "#FFE066",
            dark: "#E0A800",
            deep: "#8A6200",
            on: INK,
        },
        _ => Palette {
            fill: "#1CB0F6",
            light: "#6FD0FF",
            dark: "#1899D6",
            deep: "#0B6FA0",
            on: "#FFFFFF",
        },
    }
}

/// `text` with whitespace collapsed and every character `supported` rejects removed.
pub(crate) fn clean(text: &str, supported: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        let word: String = word.chars().filter(|c| supported(*c)).collect();
        if !word.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&word);
        }
    }
    out
}

/// Breaks `text` into at most `max_lines` lines that each `fits`, splitting words
/// only when one is wider than a line. The second value says whether text was cut,
/// in which case the last line ends with an ellipsis.
pub(crate) fn wrap(
    text: &str,
    max_lines: usize,
    fits: impl Fn(&str) -> bool,
) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if fits(&candidate) {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        for c in word.chars() {
            current.push(c);
            if current.chars().count() > 1 && !fits(&current) {
                current.pop();
                lines.push(std::mem::replace(&mut current, c.to_string()));
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() <= max_lines {
        return (lines, false);
    }
    lines.truncate(max_lines);
    if let Some(last) = lines.last_mut() {
        *last = ellipsize(last, &fits);
    }
    (lines, true)
}

fn ellipsize(line: &str, fits: impl Fn(&str) -> bool) -> String {
    let mut words: Vec<&str> = line.split(' ').collect();
    while words.len() > 1 {
        let candidate = format!("{}…", words.join(" "));
        if fits(&candidate) {
            return candidate;
        }
        words.pop();
    }
    let mut chars: Vec<char> = words.concat().chars().collect();
    loop {
        let kept: String = chars.iter().collect();
        let candidate = format!("{kept}…");
        if chars.is_empty() || fits(&candidate) {
            return candidate;
        }
        chars.pop();
    }
}

/// The first of `sizes` at which `text` fits `max_lines` uncut, or the last size, cut.
pub(crate) fn fit(
    text: &str,
    sizes: &[f32],
    max_lines: usize,
    fits: impl Fn(&str, f32) -> bool,
) -> (f32, Vec<String>) {
    let mut result = (0.0, Vec::new());
    for &size in sizes {
        let (lines, cut) = wrap(text, max_lines, |line| fits(line, size));
        result = (size, lines);
        if !cut {
            break;
        }
    }
    result
}

const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_DE: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];
const MONTHS_ES: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];
const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
const MONTHS_PT: [&str; 12] = [
    "janeiro",
    "fevereiro",
    "março",
    "abril",
    "maio",
    "junho",
    "julho",
    "agosto",
    "setembro",
    "outubro",
    "novembro",
    "dezembro",
];

/// The calendar date of Anki day `day` (days since 1970-01-01), written in `language`.
pub(crate) fn date(day: i64, language: &str) -> String {
    let Some(date) = chrono::DateTime::from_timestamp(day.saturating_mul(86_400), 0) else {
        return String::new();
    };
    let (d, m, y) = (date.day(), date.month0() as usize, date.year());
    match i18n::normalize(language) {
        "de" => format!("{d}. {} {y}", MONTHS_DE[m]),
        "es" => format!("{d} de {} de {y}", MONTHS_ES[m]),
        "fr" if d == 1 => format!("1er {} {y}", MONTHS_FR[m]),
        "fr" => format!("{d} {} {y}", MONTHS_FR[m]),
        "pt" => format!("{d} de {} de {y}", MONTHS_PT[m]),
        _ => format!("{} {d}, {y}", MONTHS_EN[m]),
    }
}

/// Up to three chips for a win's stats, in `language`.
pub(crate) fn chips(stats: &serde_json::Value, language: &str) -> Vec<String> {
    [
        ("streak", "{0} day streak"),
        ("level", "Level {0}"),
        ("reviews_today", "{0} cards"),
        ("minutes_today", "{0} minutes"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        let value = stats.get(key)?.as_i64().filter(|n| *n > 0)?;
        Some(i18n::substitute(
            &i18n::text(label, language),
            &[&value.to_string()],
        ))
    })
    .take(3)
    .collect()
}

/// `text` safe inside XML and HTML text and quoted attributes.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn font(bold: bool) -> skrifa::FontRef<'static> {
    skrifa::FontRef::new(if bold { BOLD } else { REGULAR }).expect("the embedded font parses")
}

/// Whether both embedded fonts can draw `c`.
fn supported(c: char) -> bool {
    font(false).charmap().map(c).is_some() && font(true).charmap().map(c).is_some()
}

fn width(text: &str, size: f32, bold: bool) -> f32 {
    let font = font(bold);
    let charmap = font.charmap();
    let metrics = font.glyph_metrics(
        skrifa::instance::Size::new(size),
        skrifa::instance::LocationRef::default(),
    );
    text.chars()
        .map(|c| {
            charmap
                .map(c)
                .and_then(|glyph| metrics.advance_width(glyph))
                .unwrap_or(size * 0.6)
        })
        .sum()
}

struct Svg(String);

impl Svg {
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        x: f32,
        y: f32,
        size: f32,
        bold: bool,
        fill: &str,
        anchor: &str,
        content: &str,
    ) {
        let _ = write!(
            self.0,
            r#"<text x="{x:.1}" y="{y:.1}" font-family="{FAMILY}" font-size="{size:.1}" font-weight="{}" fill="{fill}" text-anchor="{anchor}">{}</text>"#,
            if bold { 700 } else { 400 },
            escape(content)
        );
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: &str) {
        let _ = write!(
            self.0,
            r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{fill}"/>"#
        );
    }

    /// A rounded rectangle at `[x, y, width, height, radius]`.
    fn rect(&mut self, [x, y, w, h, r]: [f32; 5], fill: &str, extra: &str) {
        let _ = write!(
            self.0,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="{r:.1}" fill="{fill}"{extra}/>"#
        );
    }
}

/// A small deterministic generator, so a card's confetti stays put between renders.
struct Confetti(u64);

impl Confetti {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next()
    }
}

fn background(svg: &mut Svg, width: f32, height: f32, palette: &Palette, seed: u64) {
    let _ = write!(
        svg.0,
        r##"<defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient></defs><rect width="{width}" height="{height}" fill="url(#bg)"/>"##,
        palette.light, palette.fill
    );
    let mut random = Confetti(seed);
    let count = (width * height / 26_000.0) as usize;
    for index in 0..count {
        let x = random.between(0.0, width);
        let y = random.between(0.0, height);
        let opacity = random.between(0.25, 0.55);
        let colour = if index % 3 == 0 {
            palette.dark
        } else {
            "#FFFFFF"
        };
        match index % 3 {
            0 => {
                let r = random.between(6.0, 15.0);
                let _ = write!(
                    svg.0,
                    r#"<circle cx="{x:.1}" cy="{y:.1}" r="{r:.1}" fill="{colour}" fill-opacity="{opacity:.2}"/>"#
                );
            }
            1 => {
                let w = random.between(16.0, 30.0);
                let angle = random.between(0.0, 180.0);
                let _ = write!(
                    svg.0,
                    r#"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="10" rx="5" fill="{colour}" fill-opacity="{opacity:.2}" transform="rotate({angle:.0} {x:.1} {y:.1})"/>"#,
                    x - w / 2.0,
                    y - 5.0
                );
            }
            _ => {
                let s = random.between(10.0, 20.0);
                let t = s * 0.28;
                let _ = write!(
                    svg.0,
                    r#"<path d="M{x:.1} {:.1} Q{:.1} {:.1} {:.1} {y:.1} Q{:.1} {:.1} {x:.1} {:.1} Q{:.1} {:.1} {:.1} {y:.1} Q{:.1} {:.1} {x:.1} {:.1}Z" fill="{colour}" fill-opacity="{opacity:.2}"/>"#,
                    y - s,
                    x + t,
                    y - t,
                    x + s,
                    x + t,
                    y + t,
                    y + s,
                    x - t,
                    y + t,
                    x - s,
                    x - t,
                    y - t,
                    y - s,
                );
            }
        }
    }
}

/// A white sticker panel with a coloured lip and a soft shadow. The shadow is
/// layered translucent shapes rather than a blur filter, which would triple the render time.
fn panel(svg: &mut Svg, x: f32, y: f32, w: f32, h: f32, palette: &Palette) {
    for step in (1..=4).rev() {
        let spread = step as f32 * 5.0;
        svg.rect(
            [
                x - spread / 2.0,
                y + spread,
                w + spread,
                h + spread,
                48.0 + spread,
            ],
            "#000000",
            r#" fill-opacity="0.04""#,
        );
    }
    svg.rect([x, y + 12.0, w, h, 48.0], palette.dark, "");
    svg.rect([x, y, w, h, 48.0], "#FFFFFF", "");
}

/// Draws chips left to right from (x, y), leaving out any that would pass `max_width`.
#[allow(clippy::too_many_arguments)]
fn chip_row(
    svg: &mut Svg,
    x: f32,
    y: f32,
    height: f32,
    size: f32,
    max_width: f32,
    chips: &[String],
    palette: &Palette,
) {
    let mut left = x;
    for chip in chips {
        let w = width(chip, size, true) + height * 0.8;
        if left + w > x + max_width {
            break;
        }
        svg.rect([left, y + 5.0, w, height, height / 2.0], palette.dark, "");
        svg.rect([left, y, w, height, height / 2.0], palette.fill, "");
        svg.text(
            left + w / 2.0,
            y + height / 2.0 + size * 0.36,
            size,
            true,
            palette.on,
            "middle",
            chip,
        );
        left += w + 16.0;
    }
}

/// The host in a pill whose right edge, or centre when `centred`, is at `anchor_x`.
fn pill(
    svg: &mut Svg,
    (anchor_x, y): (f32, f32),
    height: f32,
    size: f32,
    centred: bool,
    text: &str,
    palette: &Palette,
) {
    let w = width(text, size, true) + height;
    let x = if centred {
        anchor_x - w / 2.0
    } else {
        anchor_x - w
    };
    svg.rect(
        [x, y, w, height, height / 2.0],
        "#FFFFFF",
        r#" fill-opacity="0.92""#,
    );
    svg.text(
        x + w / 2.0,
        y + height / 2.0 + size * 0.36,
        size,
        true,
        palette.deep,
        "middle",
        text,
    );
}

/// The player's avatar ring, and an initial when they have no picture.
#[allow(clippy::too_many_arguments)]
fn avatar(
    svg: &mut Svg,
    cx: f32,
    cy: f32,
    r: f32,
    ring: f32,
    ring_colour: &str,
    picture: bool,
    display: &str,
    palette: &Palette,
) {
    svg.circle(cx, cy, r + ring, ring_colour);
    if picture {
        svg.circle(cx, cy, r, "#FFFFFF");
    } else {
        svg.circle(cx, cy, r, palette.dark);
        let initial: String = display
            .chars()
            .find(|c| c.is_alphanumeric())
            .map_or_else(|| "?".into(), |c| c.to_uppercase().collect());
        svg.text(cx, cy + r * 0.36, r, true, "#FFFFFF", "middle", &initial);
    }
}

fn one_line(text: &str, sizes: &[f32], max_width: f32, bold: bool) -> (f32, String) {
    let (size, lines) = fit(text, sizes, 1, |line, size| {
        width(line, size, bold) <= max_width
    });
    (size, lines.into_iter().next().unwrap_or_default())
}

/// How many `line_height` lines fit between `top` (a first baseline) and `bottom`.
fn lines_between(top: f32, bottom: f32, line_height: f32) -> usize {
    if top > bottom {
        0
    } else {
        1 + ((bottom - top) / line_height) as usize
    }
}

struct Drawing {
    svg: String,
    art: Option<(f32, f32, f32)>,
    avatar: (f32, f32, f32),
}

fn layout(card: &Card, shape: Shape, picture: bool, art: bool) -> Drawing {
    let (w, h) = shape.size();
    let (w, h) = (w as f32, h as f32);
    let palette = palette(&card.kind);
    let mut svg = Svg(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#
    ));
    background(&mut svg, w, h, &palette, card.seed);
    let title_fits = |line: &str, size: f32, max: f32| width(line, size, true) <= max;
    let drawing = match shape {
        Shape::Square => {
            let art_rect = art.then_some((590.0, 10.0, 460.0));
            let top = if art { 360.0 } else { 250.0 };
            let bottom = 968.0;
            let avatar_circle = (112.0, 112.0, 54.0);
            avatar(
                &mut svg,
                avatar_circle.0,
                avatar_circle.1,
                avatar_circle.2,
                9.0,
                "#FFFFFF",
                picture,
                &card.display,
                &palette,
            );
            let header_width = if art { 400.0 } else { 830.0 };
            let (size, name) = one_line(&card.display, &[46.0, 38.0, 32.0], header_width, true);
            svg.text(190.0, 104.0, size, true, palette.on, "start", &name);
            svg.0.push_str(r#"<g fill-opacity="0.85">"#);
            svg.text(190.0, 150.0, 30.0, false, palette.on, "start", &card.date);
            svg.0.push_str("</g>");
            panel(&mut svg, 60.0, top, 960.0, bottom - top, &palette);
            let x = 112.0;
            let inner = 856.0;
            let mut y = if art { 490.0 } else { top + 70.0 };
            let (size, title) = fit(&card.title, &[96.0, 84.0, 72.0, 60.0], 2, |line, size| {
                title_fits(line, size, inner)
            });
            for (index, line) in title.iter().enumerate() {
                if index > 0 {
                    y += size * 1.1;
                } else {
                    y += size * 0.92;
                }
                svg.text(x, y, size, true, INK, "start", line);
            }
            let chips_top = bottom - 48.0 - 80.0;
            let body_size = 38.0;
            let first = y + size * 0.3 + 20.0 + body_size;
            let room = if card.chips.is_empty() {
                bottom - 48.0
            } else {
                chips_top - 24.0
            };
            let max_lines = lines_between(first, room, 50.0).min(3);
            let (body, _) = wrap(&card.body, max_lines, |line| {
                width(line, body_size, false) <= inner
            });
            for (index, line) in body.iter().enumerate() {
                svg.text(
                    x,
                    first + index as f32 * 50.0,
                    body_size,
                    false,
                    "#4B4B4B",
                    "start",
                    line,
                );
            }
            chip_row(
                &mut svg,
                x,
                chips_top,
                76.0,
                34.0,
                inner,
                &card.chips,
                &palette,
            );
            pill(
                &mut svg,
                (w / 2.0, 996.0),
                60.0,
                28.0,
                true,
                &card.host,
                &palette,
            );
            Drawing {
                svg: String::new(),
                art: art_rect,
                avatar: avatar_circle,
            }
        }
        Shape::Wide => {
            let art_rect = art.then_some((0.0, 70.0, 460.0));
            let (left, x) = if art { (400.0, 500.0) } else { (50.0, 110.0) };
            let right = 1150.0;
            let (top, bottom) = (40.0, 548.0);
            panel(&mut svg, left, top, right - left, bottom - top, &palette);
            let inner = right - 56.0 - x;
            let avatar_circle = (x + 32.0, top + 76.0, 32.0);
            avatar(
                &mut svg,
                avatar_circle.0,
                avatar_circle.1,
                avatar_circle.2,
                6.0,
                palette.fill,
                picture,
                &card.display,
                &palette,
            );
            let (size, name) = one_line(&card.display, &[30.0, 26.0, 22.0], inner - 84.0, true);
            svg.text(x + 84.0, top + 70.0, size, true, INK, "start", &name);
            svg.text(
                x + 84.0,
                top + 102.0,
                22.0,
                false,
                MUTED,
                "start",
                &card.date,
            );
            let mut y = top + 136.0;
            let (size, title) = fit(&card.title, &[68.0, 58.0, 50.0, 44.0], 2, |line, size| {
                title_fits(line, size, inner)
            });
            for (index, line) in title.iter().enumerate() {
                y += if index > 0 { size * 1.1 } else { size * 0.92 };
                svg.text(x, y, size, true, INK, "start", line);
            }
            let chips_top = bottom - 36.0 - 59.0;
            let body_size = 26.0;
            let first = y + size * 0.3 + 14.0 + body_size;
            let room = if card.chips.is_empty() {
                bottom - 36.0
            } else {
                chips_top - 18.0
            };
            let max_lines = lines_between(first, room, 36.0).min(3);
            let (body, _) = wrap(&card.body, max_lines, |line| {
                width(line, body_size, false) <= inner
            });
            for (index, line) in body.iter().enumerate() {
                svg.text(
                    x,
                    first + index as f32 * 36.0,
                    body_size,
                    false,
                    "#4B4B4B",
                    "start",
                    line,
                );
            }
            chip_row(
                &mut svg,
                x,
                chips_top,
                54.0,
                24.0,
                inner,
                &card.chips,
                &palette,
            );
            pill(
                &mut svg,
                (right, 568.0),
                46.0,
                22.0,
                false,
                &card.host,
                &palette,
            );
            Drawing {
                svg: String::new(),
                art: art_rect,
                avatar: avatar_circle,
            }
        }
    };
    svg.0.push_str("</svg>");
    Drawing {
        svg: svg.0,
        ..drawing
    }
}

fn options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_font_data(REGULAR.to_vec());
        fonts.load_font_data(BOLD.to_vec());
        fonts.set_sans_serif_family(FAMILY);
        usvg::Options {
            font_family: FAMILY.into(),
            fontdb: Arc::new(fonts),
            ..usvg::Options::default()
        }
    })
}

fn draw(
    target: &mut tiny_skia::Pixmap,
    image: &tiny_skia::Pixmap,
    x: f32,
    y: f32,
    size: f32,
    mask: Option<&tiny_skia::Mask>,
) {
    let scale = size / image.width().max(image.height()) as f32;
    target.draw_pixmap(
        0,
        0,
        image.as_ref(),
        &tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bicubic,
            ..Default::default()
        },
        tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, x, y),
        mask,
    );
}

/// The card as a PNG of `shape`'s size.
pub(crate) fn render(card: &Card, shape: Shape) -> Option<Vec<u8>> {
    let picture = card
        .avatar
        .as_deref()
        .and_then(|png| tiny_skia::Pixmap::decode_png(png).ok());
    let art = card.art.and_then(art_pixmap);
    let drawing = layout(card, shape, picture.is_some(), art.is_some());
    let tree = usvg::Tree::from_str(&drawing.svg, options()).ok()?;
    let (w, h) = shape.size();
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    if let (Some(art), Some((x, y, size))) = (art, drawing.art) {
        draw(&mut pixmap, art, x, y, size, None);
    }
    if let Some(picture) = &picture {
        let (cx, cy, r) = drawing.avatar;
        let mut mask = tiny_skia::Mask::new(w, h)?;
        mask.fill_path(
            &tiny_skia::PathBuilder::from_circle(cx, cy, r)?,
            tiny_skia::FillRule::Winding,
            true,
            tiny_skia::Transform::default(),
        );
        draw(&mut pixmap, picture, cx - r, cy - r, 2.0 * r, Some(&mask));
    }
    encode(&pixmap)
}

/// The opaque card as an RGB PNG. Fast compression keeps encoding well under the
/// time drawing takes, for files about a tenth larger.
fn encode(pixmap: &tiny_skia::Pixmap) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let mut rgb = Vec::with_capacity(pixmap.pixels().len() * 3);
    for pixel in pixmap.pixels() {
        let colour = pixel.demultiply();
        rgb.extend_from_slice(&[colour.red(), colour.green(), colour.blue()]);
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        &rgb,
        pixmap.width(),
        pixmap.height(),
        image::ExtendedColorType::Rgb8,
    )
    .ok()?;
    Some(out)
}

fn cache_key(card: &Card, shape: Shape) -> [u8; 32] {
    let mut hash = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    };
    field(format!("{shape:?}").as_bytes());
    field(card.kind.as_bytes());
    field(card.title.as_bytes());
    field(card.body.as_bytes());
    for chip in &card.chips {
        field(chip.as_bytes());
    }
    field(card.display.as_bytes());
    field(card.date.as_bytes());
    field(card.host.as_bytes());
    field(card.art.unwrap_or_default().as_bytes());
    field(card.avatar.as_deref().unwrap_or_default());
    field(&card.seed.to_le_bytes());
    hash.finalize().into()
}

/// [`render`], through a small cache of recent cards. The key covers every input,
/// so a changed avatar, name or language never serves an old picture.
pub(crate) fn png(card: &Card, shape: Shape) -> Option<Bytes> {
    static RENDERS: Mutex<VecDeque<([u8; 32], Bytes)>> = Mutex::new(VecDeque::new());
    let key = cache_key(card, shape);
    {
        let mut renders = RENDERS.lock().unwrap();
        if let Some(index) = renders.iter().position(|(k, _)| *k == key) {
            let entry = renders.remove(index)?;
            let bytes = entry.1.clone();
            renders.push_front(entry);
            return Some(bytes);
        }
    }
    let bytes = Bytes::from(render(card, shape)?);
    let mut renders = RENDERS.lock().unwrap();
    renders.push_front((key, bytes.clone()));
    renders.truncate(CACHED);
    Some(bytes)
}

/// The site's absolute base URL and the host shown on cards.
struct Site {
    base: String,
    host: String,
}

fn site(app: &App, headers: &HeaderMap) -> Site {
    if let Some(url) = app
        .config
        .public_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let base = url.trim_end_matches('/').to_string();
        let host = base
            .split_once("://")
            .map_or(base.as_str(), |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        return Site { base, host };
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| {
            !host.is_empty()
                && host.len() <= 253
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
        })
        .unwrap_or("localhost");
    Site {
        base: format!("http://{host}"),
        host: host.into(),
    }
}

/// A win's texts in its player's language, for the card and the page.
struct Shown {
    language: String,
    title: String,
    body: String,
}

fn shown(store: &Store, win: &Win) -> Result<Shown, Error> {
    let language = store.language(&win.user)?;
    Ok(Shown {
        title: i18n::system_text(&win.title, &language),
        body: i18n::system_text(&win.body, &language),
        language,
    })
}

fn card(app: &App, store: &Store, win: &Win, shown: &Shown, host: &str) -> Result<Card, Error> {
    Ok(Card {
        kind: win.kind.clone(),
        title: clean(&shown.title, supported),
        body: clean(&shown.body, supported),
        chips: chips(&win.stats, &shown.language),
        display: clean(&app.display(&win.user), supported),
        date: date(win.day, &shown.language),
        host: clean(host, supported),
        art: art(&store.companion(&win.user)?, &win.art),
        avatar: avatars::picture(store, &win.user)?.map(|(_, png)| png),
        seed: win.id as u64,
    })
}

fn status(code: StatusCode) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn image(card: Card, shape: Shape, cache: &'static str) -> Response {
    match tokio::task::spawn_blocking(move || png(&card, shape)).await {
        Ok(Some(bytes)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, cache),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        _ => {
            log_error!("share card rendering failed");
            status(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// What the public page of a win shows.
pub(crate) struct Page<'a> {
    pub(crate) language: &'a str,
    pub(crate) base: &'a str,
    pub(crate) token: &'a str,
    pub(crate) display: &'a str,
    pub(crate) title: &'a str,
    pub(crate) body: &'a str,
}

pub(crate) fn page_html(page: &Page) -> String {
    let language = i18n::normalize(page.language);
    let invitation = i18n::substitute(
        &i18n::text("Study with {0} on AnkiQuest", language),
        &[page.display],
    );
    let join = i18n::text("Join AnkiQuest", language);
    let heading = format!("{}: {}", page.display, page.title);
    let description = if page.body.trim().is_empty() {
        invitation.as_str()
    } else {
        page.body
    };
    let url = format!("{}/w/{}", page.base, page.token);
    format!(
        r##"<!doctype html>
<html lang="{language}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{heading}</title>
<meta name="description" content="{description}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="AnkiQuest">
<meta property="og:url" content="{url}">
<meta property="og:title" content="{heading}">
<meta property="og:description" content="{description}">
<meta property="og:image" content="{url}.wide.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:image:alt" content="{title}">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{heading}">
<meta name="twitter:description" content="{description}">
<meta name="twitter:image" content="{url}.wide.png">
<link rel="icon" href="/icon.svg">
<style>
body{{margin:0;background:#F7F7F7;color:#3C3C3C;font:18px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}}
main{{max-width:560px;margin:0 auto;padding:24px 16px 48px;text-align:center}}
img{{width:100%;height:auto;border-radius:24px;box-shadow:0 8px 24px rgba(0,0,0,.15)}}
h1{{font-size:1.6rem;margin:24px 0 8px}}
p{{margin:0 0 16px}}
a.join{{display:inline-block;margin-top:8px;padding:14px 28px;border-radius:16px;background:#58CC02;box-shadow:0 4px 0 #58A700;color:#fff;font-weight:700;text-decoration:none}}
</style>
</head>
<body>
<main>
<img src="/w/{token}.png" width="1080" height="1080" alt="{title}">
<h1>{title}</h1>
<p>{body}</p>
<p>{invitation}</p>
<a class="join" href="/">{join}</a>
</main>
</body>
</html>
"##,
        heading = escape(&heading),
        description = escape(description),
        url = escape(&url),
        title = escape(page.title),
        body = escape(page.body),
        token = escape(page.token),
        invitation = escape(&invitation),
        join = escape(&join),
    )
}

async fn public(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (token, shape) = if let Some(token) = name.strip_suffix(".wide.png") {
        (token, Some(Shape::Wide))
    } else if let Some(token) = name.strip_suffix(".png") {
        (token, Some(Shape::Square))
    } else {
        (name.as_str(), None)
    };
    if !valid_token(token) {
        return status(StatusCode::NOT_FOUND);
    }
    let site = site(&app, &headers);
    let found = (|| -> Result<Option<(Win, Shown, Option<Card>)>, Error> {
        let store = app.store.lock().unwrap();
        let Some((_, win)) = wins::by_token(&store.conn, token) else {
            return Ok(None);
        };
        if !app.is_user(&win.user) {
            return Ok(None);
        }
        let shown = shown(&store, &win)?;
        let card = match shape {
            Some(_) => Some(card(&app, &store, &win, &shown, &site.host)?),
            None => None,
        };
        Ok(Some((win, shown, card)))
    })();
    let (win, shown, card) = match found {
        Ok(Some(found)) => found,
        Ok(None) => return status(StatusCode::NOT_FOUND),
        Err(error) => {
            log_error!("share page failed: {error}");
            return status(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    match (shape, card) {
        (Some(shape), Some(card)) => image(card, shape, PUBLIC_IMAGE).await,
        _ => (
            [(header::CACHE_CONTROL, "no-store")],
            Html(page_html(&Page {
                language: &shown.language,
                base: &site.base,
                token,
                display: &app.display(&win.user),
                title: &shown.title,
                body: &shown.body,
            })),
        )
            .into_response(),
    }
}

async fn owner_card(
    State(app): State<Arc<App>>,
    Path((user, id)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return status(StatusCode::UNAUTHORIZED);
    }
    let site = site(&app, &headers);
    let found = (|| -> Result<Option<Card>, Error> {
        let store = app.store.lock().unwrap();
        let Some(win) = wins::win(&store.conn, &user, id) else {
            return Ok(None);
        };
        let shown = shown(&store, &win)?;
        Ok(Some(card(&app, &store, &win, &shown, &site.host)?))
    })();
    match found {
        Ok(Some(card)) => image(card, Shape::Square, "private, no-store").await,
        Ok(None) => status(StatusCode::NOT_FOUND),
        Err(error) => {
            log_error!(user: &user, "share card for {user} failed: {error}");
            status(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/w/{token}", get(public))
        .route("/api/wins/{user}/{id}/card.png", get(owner_card))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Config, Week, now_ms};
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::RwLock;
    use tower::ServiceExt;

    fn sample(art: Option<&'static str>, avatar: Option<Vec<u8>>, kind: &str) -> Card {
        Card {
            kind: kind.into(),
            title: "Daily goal reached!".into(),
            body: "120 cards today in 35 minutes. Café, crème brûlée — €5 · naïve…".into(),
            chips: chips(
                &serde_json::json!({"streak": 51, "level": 54, "reviews_today": 120, "minutes_today": 35}),
                "en",
            ),
            display: "Cerro Torre".into(),
            date: date(20_392, "en"),
            host: "anki.example.com".into(),
            art,
            avatar,
            seed: 17,
        }
    }

    fn avatar_png() -> Vec<u8> {
        let image = image::RgbaImage::from_fn(256, 256, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 160, 255])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn dimensions(png: &[u8]) -> (u32, u32) {
        use image::GenericImageView;
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .unwrap()
            .dimensions()
    }

    #[test]
    fn cleaning_drops_characters_the_font_lacks_and_collapses_spaces() {
        assert_eq!(clean("🎉 Cerro:\n 3 wins", supported), "Cerro: 3 wins");
        assert_eq!(
            clean("€5 · café — naïve… “ok” ‘ok’ Łódź", supported),
            "€5 · café — naïve… “ok” ‘ok’ Łódź"
        );
        assert_eq!(clean("  \u{0}\u{7}  ", supported), "");
        assert_eq!(clean("a  🎉  b", |c| c.is_ascii()), "a b");
    }

    #[test]
    fn wrapping_fills_lines_breaks_long_words_and_ellipsizes() {
        let ten = |line: &str| line.chars().count() <= 10;
        assert_eq!(wrap("", 2, ten), (vec![], false));
        assert_eq!(
            wrap("one two three four", 3, ten),
            (vec!["one two".into(), "three four".into()], false)
        );
        assert_eq!(
            wrap("abcdefghijklmnop", 3, ten),
            (vec!["abcdefghij".into(), "klmnop".into()], false)
        );
        assert_eq!(
            wrap("one two three four five six", 2, ten),
            (vec!["one two".into(), "three…".into()], true)
        );
        assert_eq!(wrap("one two", 0, ten), (vec![], true));
    }

    #[test]
    fn fitting_shrinks_until_the_text_fits_or_cuts_at_the_smallest_size() {
        let fits = |line: &str, size: f32| line.chars().count() as f32 * size <= 100.0;
        assert_eq!(
            fit("hello", &[20.0, 10.0], 1, fits),
            (20.0, vec!["hello".into()])
        );
        assert_eq!(
            fit("hello world", &[20.0, 10.0], 1, fits),
            (10.0, vec!["hello…".into()])
        );
        assert_eq!(
            fit("hello world", &[20.0, 10.0], 2, fits),
            (20.0, vec!["hello".into(), "world".into()])
        );
        assert_eq!(
            fit("helloworld", &[20.0, 10.0], 1, fits),
            (10.0, vec!["helloworld".into()])
        );
    }

    #[test]
    fn dates_and_chips_follow_the_language() {
        assert_eq!(date(20_393, "en"), "November 1, 2025");
        assert_eq!(date(20_393, "fr"), "1er novembre 2025");
        assert_eq!(date(20_392, "de-AT"), "31. Oktober 2025");
        assert_eq!(date(20_392, "es"), "31 de octubre de 2025");
        assert_eq!(date(20_392, "pt-BR"), "31 de outubro de 2025");
        let stats = serde_json::json!({"streak": 51, "level": 54, "reviews_today": 120, "minutes_today": 35});
        assert_eq!(
            chips(&stats, "en"),
            ["51 day streak", "Level 54", "120 cards"]
        );
        assert_eq!(
            chips(&stats, "de"),
            ["51 Tage Lernserie", "Level 54", "120 Karten"]
        );
        assert_eq!(
            chips(&serde_json::json!({"streak": 0, "minutes_today": 35}), "es"),
            ["35 minutos"]
        );
        assert!(chips(&serde_json::json!({}), "en").is_empty());
    }

    #[test]
    fn art_follows_the_companion_and_falls_back_to_celebrate() {
        assert_eq!(art("aki", "streak"), Some("aki-streak"));
        assert_eq!(art("aki", "missing"), Some("aki-celebrate"));
        assert_eq!(art("ankilope", "winner"), Some("ankilope-celebrate"));
        assert_eq!(art("none", "celebrate"), None);
        for name in [
            "aki-celebrate",
            "aki-streak",
            "aki-winner",
            "aki-freeze",
            "ankilope-celebrate",
        ] {
            assert!(art_png(name).is_some());
        }
    }

    #[test]
    fn every_pose_renders_both_sizes_with_and_without_a_usable_avatar() {
        let poses = [
            ("aki", "celebrate"),
            ("aki", "streak"),
            ("aki", "winner"),
            ("aki", "freeze"),
            ("aki", "missing"),
            ("ankilope", "streak"),
            ("none", "celebrate"),
        ];
        for (index, (companion, pose)) in poses.into_iter().enumerate() {
            let avatar = [None, Some(avatar_png()), Some(b"not a png".to_vec())][index % 3].clone();
            for shape in [Shape::Square, Shape::Wide] {
                let mut card = sample(art(companion, pose), avatar.clone(), "streak");
                card.seed = index as u64;
                let png = render(&card, shape).unwrap();
                assert_eq!(
                    dimensions(&png),
                    shape.size(),
                    "{companion} {pose} {shape:?}"
                );
            }
        }
    }

    #[test]
    fn long_texts_and_no_chips_still_render() {
        let mut card = sample(None, None, "unknown");
        card.title = "W".repeat(200);
        card.body = "word ".repeat(200);
        card.display = "X".repeat(100);
        card.chips.clear();
        for shape in [Shape::Square, Shape::Wide] {
            assert_eq!(dimensions(&render(&card, shape).unwrap()), shape.size());
        }
    }

    #[test]
    fn cached_renders_are_reused_only_for_identical_cards() {
        let card = sample(art("aki", "celebrate"), None, "level");
        let first = png(&card, Shape::Wide).unwrap();
        assert_eq!(first.as_ptr(), png(&card, Shape::Wide).unwrap().as_ptr());
        let mut renamed = sample(art("aki", "celebrate"), None, "level");
        renamed.display = "Hill".into();
        assert_ne!(first.as_ptr(), png(&renamed, Shape::Wide).unwrap().as_ptr());
    }

    #[test]
    fn the_page_escapes_text_and_points_previews_at_absolute_urls() {
        let html = page_html(&Page {
            language: "en",
            base: "https://anki.example.com",
            token: "abc",
            display: "<b>Cerro</b>",
            title: "\"><script>alert(1)</script>",
            body: "Tom & Jerry's",
        });
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<b>"));
        assert!(html.contains("<h1>&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;</h1>"));
        assert!(html.contains("<p>Tom &amp; Jerry&#39;s</p>"));
        for tag in [
            r#"<meta property="og:title" content="&lt;b&gt;Cerro&lt;/b&gt;: &quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;">"#,
            r#"<meta property="og:description" content="Tom &amp; Jerry&#39;s">"#,
            r#"<meta property="og:image" content="https://anki.example.com/w/abc.wide.png">"#,
            r#"<meta property="og:image:width" content="1200">"#,
            r#"<meta property="og:image:height" content="630">"#,
            r#"<meta property="og:url" content="https://anki.example.com/w/abc">"#,
            r#"<meta name="twitter:card" content="summary_large_image">"#,
        ] {
            assert!(html.contains(tag), "{tag}");
        }
        let german = page_html(&Page {
            language: "de",
            base: "https://a",
            token: "t",
            display: "Cerro",
            title: "T",
            body: "",
        });
        assert!(german.contains("Lerne mit Cerro auf AnkiQuest"));
        assert!(german.contains(r#"<html lang="de">"#));
    }

    fn fixture(private_site: bool, public_url: Option<&str>) -> (Arc<App>, PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        let mut config = serde_json::json!({
            "private_site": private_site,
            "users": {
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "hill": {"display": "Hill", "token": "hill-secret"}
            }
        });
        if let Some(url) = public_url {
            config["public_url"] = url.into();
        }
        let config: Config = serde_json::from_value(config).unwrap();
        (
            Arc::new(App {
                config,
                access: crate::access::Access::default(),
                accounts: Default::default(),
                week: Week::default(),
                store: Mutex::new(store),
                players: RwLock::new(HashMap::new()),
                standings: Default::default(),
                heartbeat: Default::default(),
            }),
            path,
        )
    }

    fn insert(app: &App, user: &str, title: &str, token: Option<&str>) -> i64 {
        let store = app.store.lock().unwrap();
        store
            .conn
            .execute(
                "insert into wins (user, kind, key, day, created_at, title, body, stats, art, link_token)
                 values (?1, 'daily_goal', ?2, 20391, ?3, ?4, '120 cards today in 35 minutes.', ?5, 'celebrate', ?6)",
                rusqlite::params![
                    user,
                    format!("goal:{title}"),
                    now_ms(),
                    title,
                    serde_json::json!({"streak": 51, "level": 54, "reviews_today": 120}).to_string(),
                    token
                ],
            )
            .unwrap();
        store.conn.last_insert_rowid()
    }

    async fn get(app: &Arc<App>, path: &str, headers: &[(&str, &str)]) -> Response {
        let mut request = Request::builder()
            .uri(path)
            .extension(ConnectInfo::<std::net::SocketAddr>(
                "192.0.2.1:40000".parse().unwrap(),
            ))
            .header("host", "anki.example.test");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        crate::router(app.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), 16 << 20)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn public_pages_and_cards_are_served_even_on_a_private_site() {
        let (app, path) = fixture(true, Some("https://anki.example.com/"));
        let id = insert(
            &app,
            "cerro",
            "<i>Daily goal reached!</i>",
            Some("tok_en-1"),
        );
        assert_eq!(
            get(&app, "/api/leaderboard", &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let response = get(&app, "/w/tok_en-1", &[]).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let html = String::from_utf8(body(response).await).unwrap();
        assert!(html.contains("<h1>&lt;i&gt;Daily goal reached!&lt;/i&gt;</h1>"));
        assert!(!html.contains("<i>"));
        assert!(html.contains(
            r#"<meta property="og:image" content="https://anki.example.com/w/tok_en-1.wide.png">"#
        ));

        for (suffix, size) in [(".png", (1080, 1080)), (".wide.png", (1200, 630))] {
            let response = get(&app, &format!("/w/tok_en-1{suffix}"), &[]).await;
            assert_eq!(response.status(), StatusCode::OK, "{suffix}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
            assert_eq!(response.headers()[header::CACHE_CONTROL], PUBLIC_IMAGE);
            assert_eq!(dimensions(&body(response).await), size);
        }

        for missing in [
            "/w/unknown",
            "/w/unknown.png",
            "/w/unknown.wide.png",
            "/w/bad%20token",
            "/w/.png",
        ] {
            let response = get(&app, missing, &[]).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{missing}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }

        app.store
            .lock()
            .unwrap()
            .conn
            .execute("update wins set link_token = null where id = ?1", [id])
            .unwrap();
        for revoked in ["/w/tok_en-1", "/w/tok_en-1.png", "/w/tok_en-1.wide.png"] {
            assert_eq!(
                get(&app, revoked, &[]).await.status(),
                StatusCode::NOT_FOUND
            );
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn the_owner_card_needs_the_owner_and_is_never_cached() {
        let (app, path) = fixture(false, None);
        let id = insert(&app, "cerro", "Daily goal reached!", None);
        let route = format!("/api/wins/cerro/{id}/card.png");
        assert_eq!(
            get(&app, &route, &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get(&app, &route, &[("authorization", "Bearer hill-secret")])
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get(
                &app,
                &format!("/api/wins/hill/{id}/card.png"),
                &[("authorization", "Bearer hill-secret")]
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        let response = get(&app, &route, &[("authorization", "Bearer cerro-secret")]).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(dimensions(&body(response).await), (1080, 1080));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn without_a_public_url_the_page_uses_the_request_host() {
        let (app, path) = fixture(false, None);
        insert(&app, "cerro", "Daily goal reached!", Some("abc"));
        let html = String::from_utf8(body(get(&app, "/w/abc", &[]).await).await).unwrap();
        assert!(html.contains(r#"content="http://anki.example.test/w/abc.wide.png""#));
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    /// Writes example cards to `target/card-examples` to look at.
    #[test]
    #[ignore]
    fn write_examples() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/card-examples");
        std::fs::create_dir_all(&dir).unwrap();
        let examples = [
            ("daily_goal", "aki", "celebrate", true),
            ("streak", "aki", "streak", false),
            ("level", "ankilope", "celebrate", true),
            ("place", "aki", "winner", true),
            ("challenge", "none", "celebrate", false),
        ];
        for (kind, companion, pose, avatar) in examples {
            let mut card = sample(art(companion, pose), avatar.then(avatar_png), kind);
            if kind == "streak" {
                card.title = "51 day streak! You keep showing up, day after day".into();
                card.body = "Fifty-one days in a row. That is a habit now.".into();
            }
            for (shape, suffix) in [(Shape::Square, "square"), (Shape::Wide, "wide")] {
                let started = std::time::Instant::now();
                let png = render(&card, shape).unwrap();
                println!(
                    "{kind} {suffix}: {:?}, {} bytes",
                    started.elapsed(),
                    png.len()
                );
                std::fs::write(dir.join(format!("{kind}-{suffix}.png")), png).unwrap();
            }
        }
    }
}
