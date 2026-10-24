# Bundled assets

| File | Source | License |
| --- | --- | --- |
| `Inter-Regular.ttf`, `Inter-Medium.ttf`, `Inter-SemiBold.ttf` | [Inter 4.1](https://github.com/rsms/inter/releases/tag/v4.1), `extras/ttf/` | SIL OFL 1.1, see `Inter-LICENSE.txt` |
| `lucide-subset.ttf` | [lucide-static 0.460.0](https://www.npmjs.com/package/lucide-static/v/0.460.0), `font/lucide.ttf` | ISC, see `Lucide-LICENSE.txt` |
| `docs/prototypes/v3/sample.jpg`, bundled by `jvid-render` (`thumb.rs`) as the thumbnail fallback frame | A 480×270 frame of the project owner's own footage, added in 0ff9086 | The owner's; ships under the repository license |

egui can't switch OpenType features on, so tabular figures are frozen into Inter's default glyphs, and both fonts are subset to what the suite uses:

```sh
pip install fonttools opentype-feature-freezer
for w in Regular Medium SemiBold; do
  pyftfeatfreeze -f tnum Inter-$w.ttf frozen-$w.ttf
  pyftsubset frozen-$w.ttf --unicodes="U+0020-007E,U+00A0-00FF,U+2013-2014,U+2018-201D,U+2022,U+2026,U+2212,U+00D7" \
    --layout-features='kern,liga,calt' --no-hinting --output-file=Inter-$w.ttf
done
# chevron-down, chevron-right, search, check, x, plus, ellipsis, circle-alert, undo-2, redo-2,
# layout-grid, list, audio-waveform, play, pause, skip-back, skip-forward, rewind, fast-forward,
# lock, eye, eye-off, mouse-pointer-2, scissors, magnet, link, zoom-in, zoom-out, scan,
# scissors-line-dashed, sliders-horizontal, mic, folder
pyftsubset lucide.ttf --unicodes="U+E071,U+E073,U+E155,U+E070,U+E1B2,U+E141,U+E0BA,U+E07B,U+E2A1,U+E2A0,U+E105,U+E10C,U+E560,U+E140,U+E132,U+E163,U+E164,U+E14B,U+E0C1,U+E10F,U+E0BE,U+E0BF,U+E1C3,U+E152,U+E2B5,U+E108,U+E1B6,U+E1B7,U+E257,U+E4EE,U+E29A,U+E11C,U+E0DC" --output-file=lucide-subset.ttf
```

To add an icon, look up its code point in lucide-static's `font/info.json`, add it to the last command, and add a constant to `icon` in `src/theme.rs`.
