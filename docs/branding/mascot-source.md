# A single source for mascot art

Keep the transparent PNG masters in `art/mascots/<mascot>/<pose>.png`.
Register each mascot and the common pose names in `art/mascots/manifest.json`.
The exporter checks that every mascot directory is registered and contains the
full set of poses. Put group illustrations in `art/mascots/scenes/` and list
their participants by mascot ID in the manifest. Put mascot-free icons in
`art/mascots/common/`. Keep each mascot's generation prompts with its masters.

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
Use the workspace's locked PNG and lossless WebP encoders and fix the resize
filter, encoder settings, and metadata handling in the exporter. Generate the
files from the manifest in sorted order. Run the exporter twice in CI and
compare bytes to verify reproducibility.

Call the exporter library from `build.rs` for the web and card PNGs. Write them
under Cargo's `OUT_DIR`, register the source files for rebuilds, and point the
`include_bytes!` calls in `src/cards.rs` and the web handlers at those outputs.
Nix builds the same locked dependencies and source tree as Cargo. The build
script runs with its local inputs. Add-on packaging invokes the CLI to put its
PNG exports in the add-on archive.

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
