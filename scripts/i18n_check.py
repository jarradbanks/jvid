#!/usr/bin/env python3
"""Seconds-long catalog check for translation-only PRs (#309), stdlib only.

Mirrors `Catalog::mismatches` in crates/suite-ui/src/i18n.rs: every catalog parses as
strings and tables, has English's keys (stubs aside) and no others, keeps English's
{placeholders}, and each plural group has `other` plus the forms the language's rule picks
(`plural_category`; `many` for exact millions is optional, it falls back to `other`).
The Rust tests stay the source of truth; this only lets catalog PRs skip the full build.
"""

import os
import re
import sys
import tomllib
from pathlib import Path

DIRS = ["crates/jvid-app/i18n", "crates/suite-ui/i18n", "crates/suite-ui/i18n/gallery"]
FORMS = ("one", "few", "many", "other")
# Forms each language's rule picks for some count below a million, from `plural_category`.
NEEDS = {"ja": {"other"}, "zh-Hans": {"other"}, "ko": {"other"}, "cs": {"one", "few", "other"}}
NEEDS |= dict.fromkeys(["pl", "ru", "uk"], {"one", "few", "many", "other"})
PLACEHOLDER = re.compile(r"\{([A-Za-z0-9_]+)\}")


def flatten(table, prefix=""):
    out = {}
    for k, v in table.items():
        key = f"{prefix}{k}"
        if isinstance(v, dict):
            out |= flatten(v, key + ".")
        elif isinstance(v, str):
            out[key] = v
        else:
            raise ValueError(f"`{key}` is a {type(v).__name__}, not text")
    return out


def base(key):
    head, _, form = key.rpartition(".")
    return head if head and form in FORMS else None


def names(text):
    return sorted(set(PLACEHOLDER.findall(text)))


def check(directory):
    """Every mismatch in one catalog directory, as `file: problem` lines."""
    tables, out = {}, []
    for path in sorted(Path(directory).glob("*.toml")):
        try:
            tables[path.stem] = flatten(tomllib.loads(path.read_text(encoding="utf-8")))
        except (tomllib.TOMLDecodeError, ValueError) as e:
            out.append(f"{path}: {e}")
    en = tables.get("en")
    if en is None:
        return out or [f"{directory}: no parsable en.toml"]
    en_groups = {base(k) for k in en} - {None}

    def group(table, b):
        return sorted({n for f in FORMS for n in names(table.get(f"{b}.{f}", ""))} - {"count"})

    for lang, table in tables.items():
        where = f"{directory}/{lang}.toml"
        for b in {base(k) for k in table} - {None}:
            if f"{b}.other" not in table:
                out.append(f"{where}: `{b}` has no `{b}.other`")
            if b in en_groups:
                for form in sorted(NEEDS.get(lang, {"one", "other"}) - {"other"}):
                    if f"{b}.{form}" not in table:
                        out.append(f"{where}: `{b}` has no `{b}.{form}`")
        if lang == "en":
            continue
        if table.keys() - {"language.name"}:  # a stub isn't translated yet
            out += [f"{where}: `{k}` is missing" for k in sorted(en.keys() - table.keys())]
        for key, text in sorted(table.items()):
            b = base(key)
            if b in en_groups:
                theirs, ours = group(en, b), group(table, b)
            elif key in en:
                theirs, ours = names(en[key]), names(text)
            else:
                out.append(f"{where}: `{key}` isn't in English")
                continue
            if theirs != ours:
                out.append(f"{where}: `{key}` has {ours}, English {theirs}")
    return sorted(set(out))


def main():
    os.chdir(Path(__file__).resolve().parent.parent)
    problems = [p for d in DIRS for p in check(d)]
    print("\n".join(problems) or f"i18n: {len(DIRS)} catalog directories OK")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
