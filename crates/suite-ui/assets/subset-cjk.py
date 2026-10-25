#!/usr/bin/env python3
"""Subsets Noto Sans CJK to each language's common characters plus what jvid's catalogs use.

    pip install fonttools==4.65.0
    python3 crates/suite-ui/assets/subset-cjk.py [ja zh-Hans ko]

Downloads the pinned Noto Sans CJK Regular faces into a cache folder (once), then writes, next
to this script, NotoSansCJK-<code>.otf with that language's common set (COMMON) and every
character of its three catalogs, and NotoSansCJK-names.otf with just the language names the
Settings picker lists. Equal characters give equal bytes, and a file whose bytes wouldn't change
is left untouched, so a catalog edit in common characters changes no font. Only these outputs are committed; jvid itself never downloads
anything. Each language face must stay within 1 MB (docs/translating.md, Fonts and binary size).
"""

import os
import subprocess
import sys
import urllib.request
from pathlib import Path

RELEASE = "https://github.com/notofonts/noto-cjk/raw/Sans2.004/Sans/OTF"
# Pan-CJK faces: each holds every CJK glyph, drawn the way its region writes Han.
SOURCES = {
    "ja": "Japanese/NotoSansCJKjp-Regular.otf",
    "zh-Hans": "SimplifiedChinese/NotoSansCJKsc-Regular.otf",
    "ko": "Korean/NotoSansCJKkr-Regular.otf",
}
# Each CJK language's `language.name`, drawn in every language by the picker.
# Each language's common set: the EUC rows (1-based) of its national standard, from row 1
# (punctuation, full-width forms, kana, jamo) through its everyday block. ja: JIS X 0208 up to
# level 1 kanji (row 47); zh-Hans: GB 2312 up to level 1 hanzi (row 55); ko: KS X 1001 up to
# the 2,350 Hangul syllables (row 40).
COMMON = {"ja": ("euc_jp", 47), "zh-Hans": ("gb2312", 55), "ko": ("euc_kr", 40)}
NAMES = "日本語简体中文한국어"
BUDGET = 1_000_000

HERE = Path(__file__).resolve().parent
CRATES = HERE.parent.parent
CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "jvid-fonts"


def source(code):
    path = CACHE / Path(SOURCES[code]).name
    if not path.exists():
        CACHE.mkdir(parents=True, exist_ok=True)
        print(f"downloading {SOURCES[code]}")
        urllib.request.urlretrieve(f"{RELEASE}/{SOURCES[code]}", path.with_suffix(".part"))
        path.with_suffix(".part").rename(path)
    return path


def catalog_text(code):
    """Every non-ASCII character of the language's catalogs (Inter draws ASCII)."""
    dirs = ["jvid-app/i18n", "suite-ui/i18n", "suite-ui/i18n/gallery"]
    text = "".join((CRATES / d / f"{code}.toml").read_text(encoding="utf-8") for d in dirs)
    return sorted({c for c in text if ord(c) > 0x7F and not c.isspace()})


def common(code):
    """Every character the codec decodes from a 2-byte code in rows 1 to the last common row."""
    codec, last = COMMON[code]
    codes = (bytes([0xA0 + row, 0xA0 + cell]) for row in range(1, last + 1) for cell in range(1, 95))
    chars = set()
    for b in codes:
        try:
            chars.add(b.decode(codec))
        except UnicodeDecodeError:
            pass  # an unassigned cell
    return chars


def subset(src, chars, out):
    """Writes the subset to `out` unless `out` already holds those exact bytes."""
    unicodes = ",".join(f"U+{ord(c):04X}" for c in chars)
    tmp = out.with_suffix(".tmp")
    # pyftsubset keeps the source's head timestamps (no recalc), so equal sets give equal bytes.
    subprocess.run(
        ["pyftsubset", str(src), f"--unicodes={unicodes}", "--layout-features=",
         "--no-hinting", "--desubroutinize", f"--output-file={tmp}"],
        check=True,
    )
    data = tmp.read_bytes()
    if out.exists() and out.read_bytes() == data:
        tmp.unlink()
        status = "unchanged"
    else:
        tmp.replace(out)
        status = "written"
    print(f"{out.name}: {len(chars)} characters, {len(data) // 1000} KB, {status}")
    return len(data)


def main(codes):
    over = []
    for code in codes:
        out = HERE / f"NotoSansCJK-{code}.otf"
        if subset(source(code), sorted(common(code) | set(catalog_text(code))), out) > BUDGET:
            over.append(out.name)
    # The names face comes from the Japanese source; these few Han shapes match across regions.
    subset(source("ja"), sorted(set(NAMES)), HERE / "NotoSansCJK-names.otf")
    if over:
        sys.exit(f"over the {BUDGET // 1000} KB budget: {', '.join(over)}")


if __name__ == "__main__":
    main(sys.argv[1:] or list(SOURCES))
