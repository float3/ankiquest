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
    # Moment art from #88: app icon moods, streak widget, celebrations.
    ("aki_icon_happy", "static/aki/icon_happy.png", 432),
    ("aki_icon_worried", "static/aki/icon_worried.png", 432),
    ("aki_icon_frozen", "static/aki/icon_frozen.png", 432),
    ("aki_icon_sleepy", "static/aki/icon_sleepy.png", 432),
    ("aki_streak_hot", "static/aki/streak_hot.png", 336),
    ("aki_streak_blazing", "static/aki/streak_blazing.png", 336),
    ("aki_streak_legend", "static/aki/streak_legend.png", 336),
    ("aki_streak_milestone", "static/aki/streak_milestone.png", 336),
    ("aki_freeze_used", "static/aki/freeze_used.png", 336),
    ("aki_comeback", "static/aki/comeback.png", 336),
    ("aki_minimum", "static/aki/minimum.png", 336),
    ("aki_goal", "static/aki/goal.png", 336),
    ("aki_achievement", "static/aki/achievement.png", 336),
    ("aki_level_up", "static/aki/level_up.png", 336),
    ("aki_deck_conquered", "static/aki/deck_conquered.png", 336),
    ("aki_leech", "static/aki/leech.png", 336),
    ("aki_sleeping", "static/aki/sleeping.png", 336),
    ("aki_at_risk", "static/aki/at_risk.png", 336),
    ("aki_backlog", "static/aki/backlog.png", 336),
    ("ankilope_backlog", "static/ankilope/backlog.png", 336),
    ("aki_place_1", "static/aki/place_1.png", 336),
    ("aki_place_2", "static/aki/place_2.png", 336),
    ("aki_place_3", "static/aki/place_3.png", 336),
    ("ankilope_place_1", "static/ankilope/place_1.png", 336),
    ("ankilope_place_2", "static/ankilope/place_2.png", 336),
    ("ankilope_place_3", "static/ankilope/place_3.png", 336),
    ("aki_nudge", "static/aki/nudge.png", 336),
    ("aki_challenge", "static/aki/challenge.png", 336),
    ("aki_challenge_won", "static/aki/challenge_won.png", 336),
    ("aki_challenge_lost", "static/aki/challenge_lost.png", 336),
    ("ankilope_nudge", "static/ankilope/nudge.png", 336),
    ("ankilope_challenge", "static/ankilope/challenge.png", 336),
    ("ankilope_challenge_won", "static/ankilope/challenge_won.png", 336),
    ("ankilope_challenge_lost", "static/ankilope/challenge_lost.png", 336),
    ("duo_friends", "static/duo/friends.png", 336),
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
