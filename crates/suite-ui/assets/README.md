# Bundled assets

| File | Source | License |
| --- | --- | --- |
| `Inter-Regular.ttf`, `Inter-Medium.ttf`, `Inter-SemiBold.ttf` | [Inter 4.1](https://github.com/rsms/inter/releases/tag/v4.1), `extras/ttf/` | SIL OFL 1.1, see `Inter-LICENSE.txt` |
| `lucide-subset.ttf` | [lucide-static 0.460.0](https://www.npmjs.com/package/lucide-static/v/0.460.0), `font/lucide.ttf` | ISC, see `Lucide-LICENSE.txt` |

egui can't switch OpenType features on, so tabular figures are frozen into Inter's default glyphs, and both fonts are subset to what the suite uses:

```sh
pip install fonttools opentype-feature-freezer
for w in Regular Medium SemiBold; do
  pyftfeatfreeze -f tnum Inter-$w.ttf frozen-$w.ttf
  pyftsubset frozen-$w.ttf --unicodes="U+0020-007E,U+00A0-00FF,U+2013-2014,U+2018-201D,U+2022,U+2026,U+2212,U+00D7" \
    --layout-features='kern,liga,calt' --no-hinting --output-file=Inter-$w.ttf
done
# chevron-down, chevron-right, search, check, x, plus, ellipsis, circle-alert
pyftsubset lucide.ttf --unicodes="U+E071,U+E073,U+E155,U+E070,U+E1B2,U+E141,U+E0BA,U+E07B" --output-file=lucide-subset.ttf
```

To add an icon, look up its code point in lucide-static's `font/info.json`, add it to the last command, and add a constant to `src/icons.rs`.
