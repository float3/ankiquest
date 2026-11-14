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

## Duo

`static/duo/friends.png` (Aki and Ankilope high-fiving) had the same faint
pixels, shown as yellow and red specks around the ears and the high-five
marks, plus isolated dark noise away from the drawing. It got the same
rule, and additionally every 8-connected group of pixels above alpha 4
whose alpha never exceeds 16 was cleared: 567 pixels in specks of at most
alpha 11, none touching the drawing. Pixels in the drawing, its outline and
the high-five marks are unchanged. Compositing over white, dark and magenta
changes no channel by more than 9/255. Hashes are in
[measurements](measurements.json).

Regenerate the matching Android WebPs with
`cargo run -p ankiquest-xtask -- export-android-art <android-checkout>`.
The remaining mascot parity, framing and canonical-source work stays in #88.
