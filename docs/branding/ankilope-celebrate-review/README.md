# Ankilope celebration framing

Issue #88 asks for more room above the ear. The revised transparent 1254px
master keeps Ankilope's joyful jumping pose, indigo scarf and satchel, blank
flashcards and confetti. At alpha above 16, the top margin grows from 6px to
84px and the bottom margin from 24px to 74px.

The exact built-in image_gen edit prompt is recorded in `../aki-prompts.json`.
The input was `static/ankilope/celebrate.png` at server commit
`2038ae27b34ad4841096764b7e9e405b1e6c0af8`. This is a generated revision, not a
pixel-identical rescale. Every client variant derives from this same master:
the card PNG stays 512px and the desktop add-on PNG stays 256px, resized with
Pillow's Lanczos filter and saved as optimized PNGs. Their decoded pixels match
those resizes exactly; [export hashes](exports.json) record the files. Android
uses the existing 336px WebP exporter.

[Before/after](review.png) shows the PNGs at 128px and 64px on light and dark
backgrounds. Both full-size composites were also inspected. See
[alpha bounds](measurements.json); very low-alpha pixels are reported separately
so raw preview fringes are not mistaken for opaque content.
