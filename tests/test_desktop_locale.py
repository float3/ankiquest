"""Desktop counts and timestamps follow Anki's selected language."""
from datetime import datetime
import unittest
from unittest.mock import patch

from addon_loader import load_in_package

language, heatmap, inbox, board, today, preferences = (
    load_in_package("desktop_locale", name)
    for name in ("language", "heatmap_ui", "inbox", "board", "today", "preferences")
)


class Locale(unittest.TestCase):
    def test_integer_plurals_for_every_supported_language(self):
        expected = {
            "en": ("0 reviews", "1 review", "2 reviews"),
            "es": ("0 repasos", "1 repaso", "2 repasos"),
            "de": ("0 Wiederholungen", "1 Wiederholung", "2 Wiederholungen"),
            "pt_PT": ("0 revisões", "1 revisão", "2 revisões"),
        }
        for code, labels in expected.items():
            with self.subTest(language=code), patch("anki.lang.current_lang", code):
                self.assertEqual(labels, tuple(heatmap._reviews(language.tr, count) for count in range(3)))

    def test_french_zero_reviews_uses_the_singular_form(self):
        with patch("anki.lang.current_lang", "fr"):
            self.assertEqual("0 révision", heatmap._reviews(language.tr, 0))
            self.assertEqual("1 révision", heatmap._reviews(language.tr, 1))
            self.assertEqual("2 révisions", heatmap._reviews(language.tr, 2))

    def test_dates_and_clocks_follow_anki_instead_of_a_fixed_format(self):
        seconds = int(datetime(2026, 10, 28, 15, 30).timestamp())
        with patch("anki.lang.current_lang", "de_DE"):
            self.assertIn("28.10.26", inbox.when(seconds))
            self.assertIn("15:30", inbox.when(seconds))
            self.assertIn("28.10.26", preferences.last_upload_text(seconds * 1000))
            self.assertEqual("15:30", board.clock(seconds * 1000))
            self.assertEqual("15:30", today.clock(seconds * 1000))
        with patch("anki.lang.current_lang", "en_US"):
            self.assertIn("10/28/26", inbox.when(seconds))
            self.assertIn("3:30", board.clock(seconds * 1000))
            self.assertIn("PM", board.clock(seconds * 1000))
        self.assertEqual("", inbox.when(None))
        self.assertEqual("", inbox.when("soon"))


if __name__ == "__main__":
    unittest.main()
