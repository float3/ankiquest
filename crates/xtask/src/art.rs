//! Writes the Android app's companion drawables from the master PNGs in `static/`.
//!
//! Each drawable is a square lossy WebP with full-quality alpha, downscaled
//! with Lanczos to about three times its largest place in the app and never
//! upscaled. `--check` writes nothing and fails when a drawable is missing
//! or has a different pixel size.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use image::imageops::{self, FilterType};
use image::{Rgba32FImage, RgbaImage};

/// Drawable folder inside an AnkiQuest-Android checkout.
const DRAWABLES: &str = "AnkiDroid/src/main/res/drawable-nodpi";

/// WebP quality for most drawables.
const QUALITY: f32 = 88.0;

/// WebP quality for the streak widget poses, which are shown large.
const WIDGET_QUALITY: f32 = 95.0;

/// Alpha plane quality; edges must stay clean on any background.
const ALPHA_QUALITY: i32 = 100;

/// Slowest, smallest libwebp encoding method.
const METHOD: i32 = 6;

/// Large and huge streak widgets show the pose full-width, up to about
/// 1100 px (AnkiQuest-Android#168), so smaller poses were upscaled.
const WIDGET: u32 = 1024;

/// One drawable: name, master PNG, square size and WebP quality.
struct Art {
    name: &'static str,
    source: &'static str,
    size: u32,
    quality: f32,
}

/// A drawable at the default quality.
const fn art(name: &'static str, source: &'static str, size: u32) -> Art {
    Art {
        name,
        source,
        size,
        quality: QUALITY,
    }
}

/// A pose the big streak widget shows, for either companion.
const fn widget(name: &'static str, source: &'static str) -> Art {
    Art {
        name,
        source,
        size: WIDGET,
        quality: WIDGET_QUALITY,
    }
}

const ART: &[Art] = &[
    widget("aki_face", "static/aki/face.png"),
    widget("ankilope_face", "static/ankilope/face.png"),
    widget("aki_celebrate", "static/aki/celebrate.png"),
    widget("aki_streak", "static/aki/streak.png"),
    widget("ankilope_celebrate", "static/ankilope/celebrate.png"),
    widget("ankilope_study", "static/ankilope/study.png"),
    widget("aki_welcome", "static/aki/welcome.png"),
    widget("aki_review", "static/aki/review.png"),
    widget("aki_freeze", "static/aki/freeze.png"),
    widget("ankilope_welcome", "static/ankilope/welcome.png"),
    art("aki_winner", "static/aki/winner.png", 168),
    // Moment art from #88: app icon moods, streak widget, celebrations.
    art("aki_icon_happy", "static/aki/icon_happy.png", 432),
    art("aki_icon_worried", "static/aki/icon_worried.png", 432),
    art("aki_icon_frozen", "static/aki/icon_frozen.png", 432),
    art("aki_icon_sleepy", "static/aki/icon_sleepy.png", 432),
    widget("aki_streak_hot", "static/aki/streak_hot.png"),
    widget("aki_streak_blazing", "static/aki/streak_blazing.png"),
    widget("aki_streak_legend", "static/aki/streak_legend.png"),
    widget("aki_streak_milestone", "static/aki/streak_milestone.png"),
    widget("aki_freeze_used", "static/aki/freeze_used.png"),
    widget("aki_comeback", "static/aki/comeback.png"),
    widget("aki_minimum", "static/aki/minimum.png"),
    widget("aki_goal", "static/aki/goal.png"),
    art("aki_achievement", "static/aki/achievement.png", 336),
    art("aki_level_up", "static/aki/level_up.png", 336),
    art("aki_deck_conquered", "static/aki/deck_conquered.png", 336),
    art("aki_leech", "static/aki/leech.png", 336),
    widget("aki_sleeping", "static/aki/sleeping.png"),
    widget("aki_at_risk", "static/aki/at_risk.png"),
    art("aki_backlog", "static/aki/backlog.png", 336),
    art("ankilope_backlog", "static/ankilope/backlog.png", 336),
    art("aki_place_1", "static/aki/place_1.png", 336),
    art("aki_place_2", "static/aki/place_2.png", 336),
    art("aki_place_3", "static/aki/place_3.png", 336),
    art("ankilope_place_1", "static/ankilope/place_1.png", 336),
    art("ankilope_place_2", "static/ankilope/place_2.png", 336),
    art("ankilope_place_3", "static/ankilope/place_3.png", 336),
    art("aki_nudge", "static/aki/nudge.png", 336),
    art("aki_challenge", "static/aki/challenge.png", 336),
    art("aki_challenge_won", "static/aki/challenge_won.png", 336),
    art("aki_challenge_lost", "static/aki/challenge_lost.png", 336),
    art("ankilope_nudge", "static/ankilope/nudge.png", 336),
    art("ankilope_challenge", "static/ankilope/challenge.png", 336),
    art(
        "ankilope_challenge_won",
        "static/ankilope/challenge_won.png",
        336,
    ),
    art(
        "ankilope_challenge_lost",
        "static/ankilope/challenge_lost.png",
        336,
    ),
    art("duo_friends", "static/duo/friends.png", 336),
];

/// Whether to write drawables or only compare their sizes.
#[derive(PartialEq)]
enum Mode {
    Export,
    Check,
}

/// Runs the exporter: `[--check] <android-checkout>`.
pub fn run(args: &[String]) -> ExitCode {
    let mut mode = Mode::Export;
    let mut android = None;
    for arg in args {
        match arg.as_str() {
            "--check" => mode = Mode::Check,
            _ if arg.starts_with('-') || android.is_some() => return usage(),
            _ => android = Some(PathBuf::from(arg)),
        }
    }
    let Some(android) = android else {
        return usage();
    };

    let result = match mode {
        Mode::Export => export(&android),
        Mode::Check => check(&android),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the usage text and fails.
fn usage() -> ExitCode {
    eprintln!("usage: xtask export-android-art [--check] <android-checkout>");
    ExitCode::FAILURE
}

/// The repository root, where the masters live.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The drawable's path relative to an Android checkout.
fn drawable(name: &str) -> String {
    format!("{DRAWABLES}/{name}.webp")
}

/// The drawable's size: `size` square, or smaller to never upscale.
fn target(width: u32, height: u32, size: u32) -> (u32, u32) {
    let scale = (f64::from(size) / f64::from(width.max(height))).min(1.0);
    let side = |n: u32| (f64::from(n) * scale).round_ties_even() as u32;
    (side(width), side(height))
}

/// Writes every drawable into the checkout.
fn export(android: &Path) -> Result<ExitCode, String> {
    let folder = android.join(DRAWABLES);
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    for art in ART {
        let master = load(&root().join(art.source))?;
        let (width, height) = target(master.width(), master.height(), art.size);
        if (width, height) != (art.size, art.size) {
            println!(
                "{}: {} is {}x{}, kept at {width}x{height}",
                art.name,
                art.source,
                master.width(),
                master.height()
            );
        }

        // Downscale, encode and write.
        let image = shrink(master, width, height);
        let bytes = encode(&image, art.quality)?;
        let path = android.join(drawable(art.name));
        fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = drawable(art.name);
        println!("{name}: {width}x{height}, {} bytes", bytes.len());
    }
    Ok(ExitCode::SUCCESS)
}

/// Compares every drawable's pixel size with what export would write.
fn check(android: &Path) -> Result<ExitCode, String> {
    let mut failed = false;
    for art in ART {
        let (width, height) = png_size(&root().join(art.source))?;
        let expected = target(width, height, art.size);
        let name = drawable(art.name);
        let Some(actual) = webp_size(&android.join(&name))? else {
            println!("missing: {name}");
            failed = true;
            continue;
        };
        if actual != expected {
            let (aw, ah) = actual;
            let (ew, eh) = expected;
            println!("{name}: {aw}x{ah}, expected {ew}x{eh}");
            failed = true;
        }
    }
    if failed {
        return Ok(ExitCode::FAILURE);
    }
    println!("all {} drawables match", ART.len());
    Ok(ExitCode::SUCCESS)
}

/// Decodes a PNG master as 8-bit RGBA.
fn load(path: &Path) -> Result<RgbaImage, String> {
    let image = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(image.into_rgba8())
}

/// A PNG's pixel size, read from its header.
fn png_size(path: &Path) -> Result<(u32, u32), String> {
    image::image_dimensions(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A WebP's pixel size, or `None` when the file does not exist.
fn webp_size(path: &Path) -> Result<Option<(u32, u32)>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let features =
        webp::BitstreamFeatures::new(&bytes).ok_or(format!("{}: not a WebP", path.display()))?;
    Ok(Some((features.width(), features.height())))
}

/// Lanczos downscale in premultiplied alpha, as Pillow does, so fully
/// transparent pixels do not bleed their colour into the edges.
fn shrink(image: RgbaImage, width: u32, height: u32) -> RgbaImage {
    if image.dimensions() == (width, height) {
        return image;
    }
    let mut linear = Rgba32FImage::from(image::DynamicImage::ImageRgba8(image));
    for pixel in linear.pixels_mut() {
        let alpha = pixel[3];
        pixel.0[..3].iter_mut().for_each(|c| *c *= alpha);
    }
    let mut small = imageops::resize(&linear, width, height, FilterType::Lanczos3);
    for pixel in small.pixels_mut() {
        unpremultiply(&mut pixel.0);
    }
    image::DynamicImage::ImageRgba32F(small).into_rgba8()
}

/// Divides colour by alpha, clamping Lanczos overshoot.
fn unpremultiply(pixel: &mut [f32; 4]) {
    let alpha = pixel[3].clamp(0.0, 1.0);
    pixel[3] = alpha;
    if alpha == 0.0 {
        pixel[..3].fill(0.0);
        return;
    }
    pixel[..3]
        .iter_mut()
        .for_each(|c| *c = (*c / alpha).clamp(0.0, 1.0));
}

/// Encodes lossy WebP with full-quality alpha.
fn encode(image: &RgbaImage, quality: f32) -> Result<Vec<u8>, String> {
    let mut config = webp::WebPConfig::new().map_err(|()| "libwebp config failed")?;
    config.lossless = 0;
    config.quality = quality;
    config.alpha_quality = ALPHA_QUALITY;
    config.method = METHOD;
    let encoder = webp::Encoder::from_rgba(image, image.width(), image.height());
    let memory = encoder
        .encode_advanced(&config)
        .map_err(|e| format!("WebP encoding failed: {e:?}"))?;
    Ok(memory.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_never_upscales() {
        assert_eq!((336, 336), target(1254, 1254, 336));
        assert_eq!((200, 100), target(200, 100, 336));
        assert_eq!((336, 168), target(1254, 627, 336));
    }

    #[test]
    fn widget_poses_are_large() {
        // Every pose the big streak widget shows, with either companion
        // (AnkiquestStreakWidget.poses, AnkiquestCompanion), and nothing else.
        let poses = [
            "aki_face",
            "ankilope_face",
            "aki_celebrate",
            "aki_streak",
            "ankilope_celebrate",
            "ankilope_study",
            "aki_welcome",
            "aki_review",
            "aki_freeze",
            "ankilope_welcome",
            "aki_streak_hot",
            "aki_streak_blazing",
            "aki_streak_legend",
            "aki_streak_milestone",
            "aki_freeze_used",
            "aki_comeback",
            "aki_minimum",
            "aki_goal",
            "aki_sleeping",
            "aki_at_risk",
        ];
        let large: Vec<_> = ART.iter().filter(|art| art.size == WIDGET).collect();
        let names: Vec<_> = large.iter().map(|art| art.name).collect();
        assert_eq!(poses.to_vec(), names);
        assert!(large.iter().all(|art| art.quality == WIDGET_QUALITY));
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = ART.iter().map(|art| art.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(ART.len(), names.len());
    }

    #[test]
    fn masters_exist() {
        for art in ART {
            assert!(root().join(art.source).is_file(), "{}", art.source);
        }
    }

    #[test]
    fn encodes_lossy_webp() {
        let image = RgbaImage::from_pixel(64, 64, image::Rgba([200, 40, 40, 128]));
        let bytes = encode(&shrink(image, 32, 32), QUALITY).unwrap();
        let features = webp::BitstreamFeatures::new(&bytes).unwrap();
        assert_eq!((32, 32), (features.width(), features.height()));
        assert!(features.has_alpha());
        assert!(matches!(
            features.format(),
            Some(webp::BitstreamFormat::Lossy)
        ));
    }

    #[test]
    fn transparent_edges_stay_clean() {
        // A red opaque half next to transparent green must not turn green.
        let mut image = RgbaImage::new(64, 64);
        for (x, _, pixel) in image.enumerate_pixels_mut() {
            *pixel = if x < 32 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 255, 0, 0])
            };
        }
        let small = shrink(image, 16, 16);
        let edge = small.get_pixel(7, 8);
        assert!(edge[1] < 8, "{edge:?}");
    }
}
