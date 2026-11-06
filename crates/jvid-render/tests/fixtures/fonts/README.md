These test-only fonts contain rectangle glyphs created for jvid and are dedicated
to the public domain under CC0-1.0. No installed fonts or external downloads are
needed to run the tests.

`primary.ttf` covers A/B at width 600. `fallback.ttf` covers A/Ω at width 900.
`alternate.ttf` has the primary family name with width 1000. `faces.ttc` contains
the primary and fallback faces at indices 0 and 1. Their overlapping A exposes
selection changes when distinct families become one private alias.
`environment.ttf` has width 1200. The test loads it under the current platform's
common fallback names to expose preemption of a private family's secondary face.

Regenerate with Python and `fonttools==4.65.0`: `python generate.py`.
