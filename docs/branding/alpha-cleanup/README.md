# Almost-transparent pixel cleanup

Local Pillow edit approved for issue #88. The five original PNG masters already
look clean when correctly composited. Some raw previews exaggerate the RGB in
pixels with extremely low alpha; this change does not redraw their contours.

For `comeback`, `freeze_used`, `streak_hot`, `streak_blazing` and `streak_legend`,
set RGBA to `(0, 0, 0, 0)` only where `0 < alpha <= 4` (at most 1.6% opacity).
All pixels above alpha 4 remain byte-for-byte unchanged. Dimensions remain
1254 x 1254. Original generation prompts still describe the artwork; no new
image was generated.

[Before/after review](review.png) shows each pose at 128px and 64px on white and
dark backgrounds. All five cleaned masters were also inspected at full size.
[Measurements](measurements.json) record source/output SHA-256 hashes and pixel
counts. Compositing over white, dark and magenta changes no channel by more
than 4/255. No erosion, color replacement or outline masking was applied.

Reproduce from the source hashes with Pillow:

```python
image = Image.open(source).convert("RGBA")
data = bytearray(image.tobytes())
for offset in range(0, len(data), 4):
    if 0 < data[offset + 3] <= 4:
        data[offset:offset + 4] = bytes(4)
Image.frombytes("RGBA", image.size, bytes(data)).save(destination)
```

Regenerate the five matching Android WebPs with `tools/export_android_art.py`.
The remaining mascot parity, framing and canonical-source work stays in #88.
