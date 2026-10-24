#!/usr/bin/env python3
"""Subsets Noto Sans CJK to what jvid's catalogs use. Run after editing a CJK catalog.

    pip install fonttools
    python3 crates/suite-ui/assets/subset-cjk.py [ja zh-Hans ko]

Downloads the pinned Noto Sans CJK Regular faces into a cache folder (once), then writes, next
to this script, NotoSansCJK-<code>.otf with every character of that language's three catalogs,
and NotoSansCJK-names.otf with just the language names the Settings picker lists. Only these
outputs are committed; jvid itself never downloads anything. Each language face must stay
within 600 KB (docs/translating.md, Fonts and binary size).
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
NAMES = "日本語简体中文한국어"
BUDGET = 600_000

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


def subset(src, chars, out):
    unicodes = ",".join(f"U+{ord(c):04X}" for c in chars)
    subprocess.run(
        ["pyftsubset", str(src), f"--unicodes={unicodes}", "--layout-features=",
         "--no-hinting", "--desubroutinize", f"--output-file={out}"],
        check=True,
    )
    size = out.stat().st_size
    print(f"{out.name}: {len(chars)} characters, {size // 1000} KB")
    return size


def main(codes):
    over = []
    for code in codes:
        out = HERE / f"NotoSansCJK-{code}.otf"
        if subset(source(code), catalog_text(code), out) > BUDGET:
            over.append(out.name)
    # The names face comes from the Japanese source; these few Han shapes match across regions.
    subset(source("ja"), sorted(set(NAMES)), HERE / "NotoSansCJK-names.otf")
    if over:
        sys.exit(f"over the {BUDGET // 1000} KB budget: {', '.join(over)}")


if __name__ == "__main__":
    main(sys.argv[1:] or list(SOURCES))
