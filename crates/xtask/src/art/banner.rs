//! Composes the README banner and the social preview from the committed
//! desktop screenshots and the duo art.
//!
//! Both follow the Android README banner (`tools/ankiquest_banner.py` in
//! AnkiQuest-Android): its blue gradient, the warm glow, Roboto title and
//! tagline, the companion beside the title. Vector parts are SVG drawn with
//! resvg; screenshots and art are downscaled with Lanczos and placed at whole
//! pixels, so every run gives the same pixels.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use image::codecs::png::{CompressionType, PngEncoder};
use image::imageops;
use image::{ImageEncoder, RgbImage, RgbaImage};
use resvg::{tiny_skia, usvg};

use super::{load, root, shrink};

const TITLE: &str = "AnkiQuest";
const TAGLINE: &str = "Study together, keep your streak";

/// Banner title and tagline sizes, as on Android; the social preview's.
const TITLE_SIZE: f32 = 66.0;
const TAGLINE_SIZE: f32 = 32.0;
const SOCIAL_TITLE_SIZE: f32 = 112.0;
const SOCIAL_TAGLINE_SIZE: f32 = 38.0;

/// Duo height and its gap to the text on the banner; duo height and left
/// margin on the social preview.
const HERO: u32 = 138;
const HERO_GAP: f32 = 14.0;
const SOCIAL_HERO: u32 = 470;
const SOCIAL_MARGIN: u32 = 80;

/// The Android banner's colours: gradient top and bottom, glow, tagline.
const TOP: &str = "#2b66d9";
const BOTTOM: &str = "#18427c";
const GLOW: &str = "#ffa200";
const TAGLINE_INK: &str = "#fff0e0";

/// Glow blur, as the Android banner's radius 120.
const GLOW_BLUR: f32 = 120.0;

/// A blurred ellipse of light: centre, radii, colour and opacity.
struct Glow {
    centre: (f32, f32),
    radii: (f32, f32),
    colour: &'static str,
    opacity: f32,
}

/// The Android banner's warm light behind the front windows (70/255).
const WARM: Glow = Glow {
    centre: (640.0, 575.0),
    radii: (300.0, 325.0),
    colour: GLOW,
    opacity: 0.275,
};

/// A white light behind the social preview's duo: warm light on the blue
/// turns grey around the art.
const SPOTLIGHT: Glow = Glow {
    centre: (350.0, 330.0),
    radii: (300.0, 280.0),
    colour: "#ffffff",
    opacity: 0.3,
};

/// Window shadow: opacity, blur and drop, as the Android phones'.
const SHADOW_OPACITY: f32 = 0.43;
const SHADOW_BLUR: f32 = 16.0;
const SHADOW_DROP: f32 = 6.0;

/// Window chrome: title bar height, corner radius, title size.
const BAR: u32 = 28;
const RADIUS: f32 = 9.0;
const BAR_TEXT: f32 = 13.0;

/// Title bar buttons, left to right: close, minimise, zoom.
const BUTTONS: [&str; 3] = ["#ff5f57", "#febc2e", "#28c840"];

/// Inputs, relative to the repository root.
const DUO: &str = "static/duo/friends.png";
const SHOTS: &str = "docs/screenshots";
const BOLD: &str = "assets/fonts/Roboto-Bold.ttf";
const REGULAR: &str = "assets/fonts/Roboto-Regular.ttf";
const FAMILY: &str = "Roboto";

/// Which image to draw.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Readme,
    Social,
}

impl Kind {
    /// Pixel size: the Android banner's, or GitHub's social preview.
    fn size(self) -> (u32, u32) {
        match self {
            Kind::Readme => (1280, 741),
            Kind::Social => (1280, 640),
        }
    }

    /// Where the image is committed, relative to the repository root.
    fn path(self) -> &'static str {
        match self {
            Kind::Readme => "docs/graphics/banner_readme.png",
            Kind::Social => "docs/graphics/social_preview.png",
        }
    }
}

/// One desktop window: screenshot, title, top-left corner, width and the
/// largest screenshot height shown, in banner pixels.
struct Window {
    shot: &'static str,
    title: &'static str,
    x: i32,
    y: i32,
    width: u32,
    max_height: u32,
}

/// Back to front, so nearer windows cover the ones behind them: Progress
/// and Today high at the sides, Friends and Inbox lower in the middle, and
/// Today, the add-on's main window, in front.
const WINDOWS: &[Window] = &[
    Window {
        shot: "desktop-progress.png",
        title: "Progress",
        x: 24,
        y: 178,
        width: 440,
        max_height: 600,
    },
    Window {
        shot: "desktop-friends.png",
        title: "Friends",
        x: 168,
        y: 318,
        width: 440,
        max_height: 600,
    },
    Window {
        shot: "desktop-inbox.png",
        title: "Inbox",
        x: 620,
        y: 318,
        width: 440,
        max_height: 600,
    },
    Window {
        shot: "desktop-today-light.png",
        title: "Today",
        x: 870,
        y: 150,
        width: 386,
        max_height: 600,
    },
];

/// Runs the command: `[--social] [--out <file>]`.
pub(super) fn run(args: &[String]) -> ExitCode {
    let mut kind = Kind::Readme;
    let mut out = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--social" => kind = Kind::Social,
            "--out" => match args.next() {
                Some(path) => out = Some(PathBuf::from(path)),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    let out = out.unwrap_or_else(|| root().join(kind.path()));

    match render(kind).and_then(|image| save(&image, &out)) {
        Ok(()) => {
            println!("{}", out.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the usage text and fails.
fn usage() -> ExitCode {
    eprintln!("usage: xtask banner [--social] [--out <file>]");
    ExitCode::FAILURE
}

/// Draws the image.
fn render(kind: Kind) -> Result<RgbImage, String> {
    let fonts = fonts()?;
    let (width, height) = kind.size();
    let mut canvas = tiny_skia::Pixmap::new(width, height).ok_or("empty canvas")?;
    let duo = trim(load(&root().join(DUO))?);

    match kind {
        Kind::Readme => readme(&mut canvas, &fonts, duo)?,
        Kind::Social => social(&mut canvas, &fonts, duo)?,
    }
    Ok(rgb(&canvas))
}

/// The README banner: duo beside title and tagline, desktop windows below.
fn readme(canvas: &mut tiny_skia::Pixmap, fonts: &Fonts, duo: RgbaImage) -> Result<(), String> {
    background(canvas, fonts, &WARM)?;
    for window in WINDOWS {
        frame(canvas, fonts, window)?;
    }

    // Duo, title and tagline centred together, as Aki is on Android.
    let hero = to_height(duo, HERO);
    let title = svg_text(TITLE, TITLE_SIZE, "bold", "white", 0.0, 0.0);
    let tagline = svg_text(TAGLINE, TAGLINE_SIZE, "normal", TAGLINE_INK, 0.0, 0.0);
    let text = width(fonts, &title)?.max(width(fonts, &tagline)?);
    let centre = canvas.width() as f32 / 2.0 + (hero.width() as f32 + HERO_GAP) / 2.0;
    let left = (centre - text / 2.0 - HERO_GAP - hero.width() as f32).round();
    picture(canvas, &hero, left as i32, 4);
    overlay(
        canvas,
        fonts,
        &format!(
            "{}{}",
            svg_text(TITLE, TITLE_SIZE, "bold", "white", centre, 67.0),
            svg_text(TAGLINE, TAGLINE_SIZE, "normal", TAGLINE_INK, centre, 107.0),
        ),
    )
}

/// The social preview: the duo large on the left, title and tagline right.
fn social(canvas: &mut tiny_skia::Pixmap, fonts: &Fonts, duo: RgbaImage) -> Result<(), String> {
    background(canvas, fonts, &SPOTLIGHT)?;
    let hero = to_height(duo, SOCIAL_HERO);
    let top = (canvas.height() - hero.height()) / 2;
    picture(canvas, &hero, SOCIAL_MARGIN as i32, top as i32);

    // Text centred in the space right of the duo.
    let centre = (SOCIAL_MARGIN + hero.width() + canvas.width()) as f32 / 2.0;
    overlay(
        canvas,
        fonts,
        &format!(
            "{}{}",
            svg_text(TITLE, SOCIAL_TITLE_SIZE, "bold", "white", centre, 318.0),
            svg_text(
                TAGLINE,
                SOCIAL_TAGLINE_SIZE,
                "normal",
                TAGLINE_INK,
                centre,
                382.0
            ),
        ),
    )
}

/// The gradient and a glow.
fn background(canvas: &mut tiny_skia::Pixmap, fonts: &Fonts, glow: &Glow) -> Result<(), String> {
    let (w, h) = (canvas.width(), canvas.height());
    let ((cx, cy), (rx, ry)) = (glow.centre, glow.radii);
    let (colour, opacity) = (glow.colour, glow.opacity);
    overlay(
        canvas,
        fonts,
        &format!(
            r#"<defs>
<linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">
<stop offset="0" stop-color="{TOP}"/><stop offset="1" stop-color="{BOTTOM}"/>
</linearGradient>
<filter id="glow" filterUnits="userSpaceOnUse" x="0" y="0" width="{w}" height="{h}">
<feGaussianBlur stdDeviation="{GLOW_BLUR}"/>
</filter>
</defs>
<rect width="{w}" height="{h}" fill="url(#sky)"/>
<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="{colour}" fill-opacity="{opacity}" filter="url(#glow)"/>"#
        ),
    )
}

/// One window: shadow, chrome with title, and the screenshot inside.
fn frame(canvas: &mut tiny_skia::Pixmap, fonts: &Fonts, window: &Window) -> Result<(), String> {
    let shot = load(&root().join(SHOTS).join(window.shot))?;
    let scaled = to_width(shot, window.width);
    let shown = scaled.height().min(window.max_height);
    let shot = imageops::crop_imm(&scaled, 0, 0, scaled.width(), shown).to_image();
    let (x, y, w, h) = (window.x, window.y, window.width, BAR + shown);

    // Shadow and chrome behind the screenshot.
    let buttons: String = BUTTONS
        .iter()
        .enumerate()
        .map(|(i, colour)| {
            let cx = x as f32 + 16.0 + 18.0 * i as f32;
            let cy = y as f32 + BAR as f32 / 2.0;
            format!(r#"<circle cx="{cx}" cy="{cy}" r="5.5" fill="{colour}"/>"#)
        })
        .collect();
    let title = svg_text(
        window.title,
        BAR_TEXT,
        "normal",
        "#3c4043",
        x as f32 + w as f32 / 2.0,
        y as f32 + BAR as f32 / 2.0 + 4.5,
    );
    let drop = y as f32 + SHADOW_DROP;
    overlay(
        canvas,
        fonts,
        &format!(
            r##"<defs><filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
<feGaussianBlur stdDeviation="{SHADOW_BLUR}"/></filter></defs>
<rect x="{x}" y="{drop}" width="{w}" height="{h}" rx="{RADIUS}" fill="black" fill-opacity="{SHADOW_OPACITY}" filter="url(#shadow)"/>
<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{RADIUS}" fill="#e3e6ea"/>
{buttons}{title}"##
        ),
    )?;

    // The screenshot, its lower corners rounded like the window's.
    let mask = corners(fonts, (canvas.width(), canvas.height()), (x, y, w, h))?;
    let top = y + BAR as i32;
    let pixmap = pixmap(&shot)?;
    canvas.draw_pixmap(
        x,
        top,
        pixmap.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        Some(&mask),
    );

    // A thin outline keeps light windows apart where they overlap.
    overlay(
        canvas,
        fonts,
        &format!(
            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{RADIUS}" fill="none" stroke="black" stroke-opacity="0.35"/>"#,
            x as f32 + 0.5,
            y as f32 + 0.5,
            w - 1,
            h - 1,
        ),
    )
}

/// A canvas-sized mask of the window's rounded rectangle.
fn corners(
    fonts: &Fonts,
    size: (u32, u32),
    rect: (i32, i32, u32, u32),
) -> Result<tiny_skia::Mask, String> {
    let (x, y, w, h) = rect;
    let mut shape = tiny_skia::Pixmap::new(size.0, size.1).ok_or("empty mask")?;
    overlay(
        &mut shape,
        fonts,
        &format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{RADIUS}"/>"#),
    )?;
    Ok(tiny_skia::Mask::from_pixmap(
        shape.as_ref(),
        tiny_skia::MaskType::Alpha,
    ))
}

/// The art cropped to its visible pixels.
fn trim(image: RgbaImage) -> RgbaImage {
    let visible: Vec<(u32, u32)> = image
        .enumerate_pixels()
        .filter(|(_, _, pixel)| pixel[3] > 0)
        .map(|(x, y, _)| (x, y))
        .collect();
    let Some(left) = visible.iter().map(|p| p.0).min() else {
        return image;
    };
    let right = visible.iter().map(|p| p.0).max().unwrap_or(left);
    let top = visible.iter().map(|p| p.1).min().unwrap_or(0);
    let bottom = visible.iter().map(|p| p.1).max().unwrap_or(top);
    imageops::crop_imm(&image, left, top, right - left + 1, bottom - top + 1).to_image()
}

/// Downscales to `width`, keeping the aspect ratio.
fn to_width(image: RgbaImage, width: u32) -> RgbaImage {
    let ratio = f64::from(image.height()) / f64::from(image.width());
    let height = (f64::from(width) * ratio).round() as u32;
    shrink(image, width, height)
}

/// Downscales to `height`, keeping the aspect ratio.
fn to_height(image: RgbaImage, height: u32) -> RgbaImage {
    let ratio = f64::from(image.width()) / f64::from(image.height());
    let width = (f64::from(height) * ratio).round() as u32;
    shrink(image, width, height)
}

/// Draws `image` with its top-left corner at whole pixels.
fn picture(canvas: &mut tiny_skia::Pixmap, image: &RgbaImage, x: i32, y: i32) {
    let Ok(pixmap) = pixmap(image) else {
        return;
    };
    canvas.draw_pixmap(
        x,
        y,
        pixmap.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
}

/// An RGBA image as a premultiplied pixmap.
fn pixmap(image: &RgbaImage) -> Result<tiny_skia::Pixmap, String> {
    let mut pixmap = tiny_skia::Pixmap::new(image.width(), image.height()).ok_or("empty image")?;
    for (target, source) in pixmap.pixels_mut().iter_mut().zip(image.pixels()) {
        let [r, g, b, a] = source.0;
        *target = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    Ok(pixmap)
}

/// The opaque canvas as RGB.
fn rgb(canvas: &tiny_skia::Pixmap) -> RgbImage {
    let bytes = canvas
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let colour = pixel.demultiply();
            [colour.red(), colour.green(), colour.blue()]
        })
        .collect();
    RgbImage::from_raw(canvas.width(), canvas.height(), bytes).unwrap_or_default()
}

/// Writes a compact PNG.
fn save(image: &RgbImage, path: &Path) -> Result<(), String> {
    let mut bytes = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Best, Default::default());
    encoder
        .write_image(
            image,
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Roboto, loaded once per image.
type Fonts = Arc<usvg::fontdb::Database>;

/// Loads the committed Roboto faces.
fn fonts() -> Result<Fonts, String> {
    let mut fonts = usvg::fontdb::Database::new();
    for face in [BOLD, REGULAR] {
        let path = root().join(face);
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        fonts.load_font_data(bytes);
    }
    fonts.set_sans_serif_family(FAMILY);
    Ok(Arc::new(fonts))
}

/// Parses an SVG fragment the size of the canvas.
fn tree(fonts: &Fonts, size: (u32, u32), body: &str) -> Result<usvg::Tree, String> {
    let (w, h) = size;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>"#
    );
    let options = usvg::Options {
        font_family: FAMILY.into(),
        fontdb: fonts.clone(),
        ..usvg::Options::default()
    };
    usvg::Tree::from_str(&svg, &options).map_err(|e| format!("SVG: {e}"))
}

/// Draws an SVG fragment over the canvas.
fn overlay(canvas: &mut tiny_skia::Pixmap, fonts: &Fonts, body: &str) -> Result<(), String> {
    let tree = tree(fonts, (canvas.width(), canvas.height()), body)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut canvas.as_mut(),
    );
    Ok(())
}

/// The drawn width of a text fragment.
fn width(fonts: &Fonts, text: &str) -> Result<f32, String> {
    let tree = tree(fonts, (1, 1), text)?;
    Ok(tree.root().abs_bounding_box().width())
}

/// A centred line of Roboto with its baseline at `y`.
fn svg_text(text: &str, size: f32, weight: &str, fill: &str, x: f32, y: f32) -> String {
    format!(
        r#"<text x="{x}" y="{y}" font-family="{FAMILY}" font-size="{size}" font-weight="{weight}" fill="{fill}" text-anchor="middle">{text}</text>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Largest channel difference allowed against the committed image, for
    /// float rounding on other machines.
    const TOLERANCE: u8 = 2;

    /// GitHub's limit for a social preview.
    const SOCIAL_LIMIT: u64 = 1_000_000;

    /// Renders `kind` and compares it with the committed file.
    fn matches_committed(kind: Kind) {
        let image = render(kind).unwrap();
        assert_eq!(kind.size(), image.dimensions());
        let path = root().join(kind.path());
        let committed = image::open(&path).unwrap().into_rgb8();
        assert_eq!(image.dimensions(), committed.dimensions());
        let worst = image
            .as_raw()
            .iter()
            .zip(committed.as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= TOLERANCE,
            "{} is stale: run `cargo run -p ankiquest-xtask -- banner{}`",
            kind.path(),
            if kind == Kind::Social {
                " --social"
            } else {
                ""
            }
        );
    }

    #[test]
    fn readme_banner_is_current() {
        matches_committed(Kind::Readme);
    }

    #[test]
    fn social_preview_is_current() {
        matches_committed(Kind::Social);
        let bytes = fs::metadata(root().join(Kind::Social.path()))
            .unwrap()
            .len();
        assert!(bytes < SOCIAL_LIMIT, "{bytes} bytes");
    }

    #[test]
    fn renders_are_identical() {
        assert_eq!(render(Kind::Social).unwrap(), render(Kind::Social).unwrap());
    }

    #[test]
    fn screenshots_exist() {
        for window in WINDOWS {
            assert!(
                root().join(SHOTS).join(window.shot).is_file(),
                "{}",
                window.shot
            );
        }
    }

    #[test]
    fn trim_drops_transparent_margins() {
        let mut image = RgbaImage::new(10, 8);
        image.put_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        image.put_pixel(6, 5, image::Rgba([1, 2, 3, 9]));
        assert_eq!((4, 4), trim(image).dimensions());
    }
}
