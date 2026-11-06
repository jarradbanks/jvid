"""Regenerate tiny, self-contained shaping fixtures with fonttools 4.65.0."""

from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTCollection

ROOT = Path(__file__).parent


def face(name, family, characters, width):
    builder = FontBuilder(1000, isTTF=True)
    order = [".notdef", "space"] + [f"uni{ord(c):04X}" for c in characters]
    builder.setupGlyphOrder(order)
    builder.setupCharacterMap({32: "space", **{ord(c): f"uni{ord(c):04X}" for c in characters}})
    glyphs = {}
    for glyph in order:
        pen = TTGlyphPen(None)
        if glyph != "space":
            pen.moveTo((50, 0))
            pen.lineTo((width - 50, 0))
            pen.lineTo((width - 50, 700))
            pen.lineTo((50, 700))
            pen.closePath()
        glyphs[glyph] = pen.glyph()
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({glyph: (width, 0) for glyph in order})
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable({
        "familyName": family,
        "styleName": "Regular",
        "uniqueFontIdentifier": name,
        "fullName": name,
        "psName": name,
    })
    builder.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
    builder.setupPost()
    builder.setupMaxp()
    builder.setupHead(created=2082844800, modified=2082844800)
    builder.font.recalcTimestamp = False
    builder.save(ROOT / f"{name}.ttf")
    return builder.font


primary = face("primary", "Jvid Fixture Primary", "AB", 600)
fallback = face("fallback", "Jvid Fixture Fallback", "AΩ", 900)
face("alternate", "Jvid Fixture Primary", "AB", 1000)
face("environment", "Noto Sans", "AΩ", 1200)
collection = TTCollection()
collection.fonts = [primary, fallback]
collection.save(ROOT / "faces.ttc")
