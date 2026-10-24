"""Each way `i18n_check.py` must fail (#309): `python3 -m unittest discover scripts`."""

import tempfile
import unittest
from pathlib import Path

from i18n_check import check

EN = 'greet = "Hi {name}"\n[clips]\none = "{count} clip"\nother = "{count} clips"\n[language]\nname = "English"\n'
RU = 'greet = "Привет, {name}"\n[clips]\none = "{count} клип"\nfew = "{count} клипа"\nmany = "{count} клипов"\nother = "{count} клипа"\n[language]\nname = "Русский"\n'


def run(**sources):
    with tempfile.TemporaryDirectory() as d:
        for lang, text in sources.items():
            Path(d, f"{lang.replace('_', '-')}.toml").write_text(text, encoding="utf-8")
        return [line.split(": ", 1)[1] for line in check(d)]


class Check(unittest.TestCase):
    def test_good_catalogs_and_stubs_pass(self):
        self.assertEqual(run(en=EN, ru=RU, pt_BR='[language]\nname = "Português (Brasil)"\n'), [])

    def test_missing_key(self):
        self.assertEqual(run(en=EN, ru=RU.replace('greet = "Привет, {name}"\n', "")), ["`greet` is missing"])

    def test_renamed_placeholder(self):
        self.assertEqual(run(en=EN, ru=RU.replace("{name}", "{nmae}")), ["`greet` has ['nmae'], English ['name']"])

    def test_toml_syntax_error(self):
        (problem,) = run(en=EN, ru="oops = \n" + RU)
        self.assertIn("Invalid", problem)

    def test_non_text_value(self):
        self.assertEqual(run(en=EN, ru="n = 3\n" + RU), ["`n` is a int, not text"])

    def test_missing_plural_category(self):
        self.assertEqual(run(en=EN, ru=RU.replace('few = "{count} клипа"\n', "")), ["`clips` has no `clips.few`"])
        self.assertEqual(run(en=EN.replace('other = "{count} clips"\n', "")), ["`clips` has no `clips.other`"])

    def test_extra_key(self):
        self.assertEqual(run(en=EN, ru='extra = "x"\n' + RU), ["`extra` isn't in English"])


if __name__ == "__main__":
    unittest.main()
