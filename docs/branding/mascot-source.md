# A single source for mascot art

Keep the transparent PNG masters in `art/mascots/<mascot>/<pose>.png`. Use the same pose names for Aki and Ankilope. Put shared scenes under `duo` and mascot-free icons under `common`. Keep generation prompts next to the masters.

A manifest maps each pose to the existing web URL, add-on filename, card filename and Android resource name. It records each export's size and format. The source check compares the complete Aki and Ankilope pose sets before publishing a bundle.

```text
PNG masters and manifest
    |
    +-- web PNGs at the existing URLs
    +-- add-on and card PNGs
    +-- versioned Android bundle of WebP drawables
```

## Build inputs

Extend the current exporter to produce all three outputs with pinned image tooling. Generate the web, add-on and card files during packaging. The Rust build needs the card PNGs before compiling `src/cards.rs`, which embeds them in the binary. [Current exporter](../../tools/export_android_art.py).

Android pins the bundle version and SHA-256. Its build extracts the bundle into a generated `drawable-nodpi` source set. Cache the verified bundle so a prepared checkout can build offline. Check resource names and dimensions against the manifest from that bundle.

## Merge order

1. Merge the Android drawables first, then the server PNG masters. [Android PR #176](https://github.com/float3/AnkiQuest-Android/pull/176), [server PR #208](https://github.com/float3/ankiquest/pull/208).
2. Merge the server exporter and manifest once they reproduce the current filenames, dimensions and public URLs. Publish the first pinned Android bundle after its pose parity and transparency checks pass.
3. Merge Android's bundle consumer after the bundle is available. Verify its resources and offline build before removing the checked-in WebP copies.
4. Remove the committed web, add-on and card copies after each packaging path produces and verifies its own outputs. Keep existing web URLs working through the generated files.

Review the generated assets at native size and 64 px on light and dark backgrounds. Check neutral icons at 24 px and the launcher art on a device.
