"""Write the Android app's companion drawables from the master PNGs in static/.

    python tools/export_android_art.py <android-checkout>
    python tools/export_android_art.py --check <android-checkout>

Each drawable is a square lossy WebP (quality 88, full-quality alpha), downscaled with
Lanczos to about three times its largest place in the app and never upscaled. --check
writes nothing and exits 1 when a drawable is missing or has a different pixel size.
"""

import argparse
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
DRAWABLES = Path("AnkiDroid/src/main/res/drawable-nodpi")
QUALITY = 88

ART = [
    ("aki_face", "static/aki/face.png", 432),
    ("ankilope_face", "static/ankilope/face.png", 192),
    ("aki_celebrate", "static/aki/celebrate.png", 336),
    ("aki_streak", "static/aki/streak.png", 336),
    ("ankilope_celebrate", "static/ankilope/celebrate.png", 336),
    ("ankilope_study", "static/ankilope/study.png", 336),
    ("aki_welcome", "static/aki/welcome.png", 240),
    ("aki_review", "static/aki/review.png", 240),
    ("aki_freeze", "static/aki/freeze.png", 240),
    ("ankilope_welcome", "static/ankilope/welcome.png", 240),
    ("aki_winner", "static/aki/winner.png", 168),
]


def expected_size(master: Image.Image, size: int) -> tuple[int, int]:
    """The drawable's size: `size` square, or the master's own size when that is smaller."""
    width, height = master.size
    scale = min(1.0, size / max(width, height))
    return round(width * scale), round(height * scale)


def export(android: Path) -> int:
    for name, source, size in ART:
        master = Image.open(ROOT / source).convert("RGBA")
        target = expected_size(master, size)
        if target != (size, size):
            print(f"{name}: {source} is {master.size[0]}x{master.size[1]}, kept at {target[0]}x{target[1]}")
        image = master.resize(target, Image.Resampling.LANCZOS) if target != master.size else master
        path = android / DRAWABLES / f"{name}.webp"
        path.parent.mkdir(parents=True, exist_ok=True)
        image.save(path, "WEBP", quality=QUALITY, alpha_quality=100, method=6)
        print(f"{path.relative_to(android)}: {target[0]}x{target[1]}, {path.stat().st_size} bytes")
    return 0


def check(android: Path) -> int:
    failed = False
    for name, source, size in ART:
        target = expected_size(Image.open(ROOT / source), size)
        path = android / DRAWABLES / f"{name}.webp"
        if not path.exists():
            print(f"missing: {path.relative_to(android)}")
            failed = True
            continue
        actual = Image.open(path).size
        if actual != target:
            print(f"{path.relative_to(android)}: {actual[0]}x{actual[1]}, expected {target[0]}x{target[1]}")
            failed = True
    if not failed:
        print(f"all {len(ART)} drawables match")
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("android", type=Path, help="an AnkiQuest-Android checkout")
    parser.add_argument("--check", action="store_true", help="only compare pixel sizes")
    args = parser.parse_args()
    return check(args.android) if args.check else export(args.android)


if __name__ == "__main__":
    sys.exit(main())
