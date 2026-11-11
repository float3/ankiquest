# A single source for mascot art

Keep the transparent PNG masters in `art/mascots/<mascot>/<pose>.png`.
Register each mascot and its required poses in `art/mascots/manifest.json`.
Start with the shared core that every mascot already has, plus each mascot's
existing poses; expand that core as the missing art is completed. Check that
every registered mascot has its declared poses. Reserve `scenes/` for group
illustrations with participants listed by mascot ID, and `common/` for icons
without a mascot. Exclude those reserved folders from the directory-registration
check. Keep each mascot's generation prompts with its masters. Size variants
such as `face-192` and `face-32` are exports of one face master.

The manifest records each export's dimensions, format, and destination. Keep
existing web URLs, add-on filenames, card keys and Android resource names.

```text
PNG masters and manifest
    |
    +-- Rust exporter --> web, add-on and card PNGs
    |
    +-- Rust exporter --> Android WebPs and SHA-256 manifest
```

## Exporter and packaging

Add a Rust workspace crate in `tools/mascot-export/` with a CLI and a library.
Enable the existing `image` crate's `webp` feature for lossless WebP output and
commit the resulting dependency lockfile. Fix the resize filter, encoder
settings, and metadata handling in the exporter. Generate the
files from the manifest in sorted order. Run the exporter twice in CI and
compare bytes to verify reproducibility.

Call the exporter library from `build.rs` for the web and card PNGs. Write them
under Cargo's `OUT_DIR`, register the source files for rebuilds, and point the
`include_bytes!` calls in `src/cards.rs` and the web handlers at those outputs.
Nix builds the same locked dependencies and source tree as Cargo. The build
script runs with its local inputs. Keep the generated copies in `static/aki/`,
`static/ankilope/`, `assets/cards/`, `addon/art/` and `addon/*_face.png` committed
and SHA-256 checked against the manifest, so runtime consumers and development
add-ons work straight from a checkout. Add-on packaging regenerates and checks
those same PNGs before adding them to the archive. Remove a committed copy
only after every consumer generates or reads its replacement. Extend
`setWebExports` in `addon/__init__.py` from the registered mascot list when
adding another mascot; its current expression names only Aki and Ankilope.

## Android drawables

Keep the generated WebPs committed in Android's `drawable-nodpi` so a checkout
can build offline. Record the server source commit and a SHA-256 for each
drawable in an Android manifest. Whoever changes the masters runs the locked
exporter and updates the drawables and manifest in a companion PR. Android CI
regenerates the WebPs from the pinned server commit and checks the complete
resource list, dimensions, and hashes against the committed files. Gradle
consumes the committed resources directly.

Review generated assets at native size and 64 px on light and dark backgrounds.
Check neutral icons at 24 px and the launcher art on a device.
