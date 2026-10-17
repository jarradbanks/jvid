"""Create labels, milestones and issues on GitHub from spec.py.

usage: python3 create.py [--dry-run] [--update KEY [KEY ...]]
Writes created.json (key -> number/id) next to this script. Re-running skips issues and blocked-by links
already in created.json. --update re-renders the title, body, labels and milestone of existing issues from the spec.
"""
import json
import pathlib
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import spec  # noqa: E402

REPO = "jarradbanks/jvid"
HERE = pathlib.Path(__file__).parent
STATE = HERE / "created.json"
DRY = "--dry-run" in sys.argv
UPDATE = sys.argv[sys.argv.index("--update") + 1:] if "--update" in sys.argv else []


def gh(args, payload=None):
    cmd = ["gh", "api", *args]
    if payload is not None:
        cmd += ["--input", "-"]
    r = subprocess.run(cmd, input=json.dumps(payload) if payload is not None else None,
                       capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(args)}: {r.stderr.strip() or r.stdout.strip()}")
    return json.loads(r.stdout) if r.stdout.strip() else None


def validate():
    seen = set()
    for i in spec.ISSUES:
        assert i["key"] not in seen, f"duplicate key {i['key']}"
        for d in i["deps"]:
            assert d in seen, f"{i['key']} depends on {d}, which is not defined earlier"
        assert i["ms"] is None or i["ms"] in spec.MILESTONES, i["key"]
        for a in i["areas"]:
            assert a in spec.AREAS, f"{i['key']}: unknown area {a}"
        seen.add(i["key"])


def body(i, numbers):
    parts = [i["summary"]]
    if i["scope"]:
        parts.append("## Scope\n" + "\n".join(f"- [ ] {s}" for s in i["scope"]))
    if i["ac"]:
        parts.append("## Acceptance criteria\n" + "\n".join(f"- {a}" for a in i["ac"]))
    if i["ms"] is not None:
        deps = "\n".join(f"- #{numbers[d]}" for d in i["deps"]) or "None"
        parts.append("## Depends on\n" + deps)
    if i["refs"]:
        parts.append("## References\n" + "\n".join(f"- {r}" for r in i["refs"]))
    return "\n\n".join(parts)


def labels_for(i):
    out = [f"area:{a}" for a in i["areas"]]
    if i["size"]:
        out.append(f"size:{i['size']}")
    return out + i["labels"]


def payload_for(i, numbers, state):
    payload = {"title": i["title"], "body": body(i, numbers), "labels": labels_for(i)}
    if i["ms"] is not None:
        payload["milestone"] = state["milestones"][i["ms"]]
    return payload


def main():
    validate()
    by_key = {i["key"]: i for i in spec.ISSUES}
    unknown = [k for k in UPDATE if k not in by_key]
    assert not unknown, f"--update: unknown keys {unknown}"
    print(f"spec ok: {len(spec.ISSUES)} issues")
    if DRY:
        fake = {i["key"]: n for n, i in enumerate(spec.ISSUES, 1)}
        print(body(spec.ISSUES[13], fake))
        return

    state = json.loads(STATE.read_text()) if STATE.exists() else {"milestones": {}, "issues": {}}

    for name, (color, desc) in ({f"area:{k}": v for k, v in spec.AREAS.items()}
                                | {f"size:{k}": v for k, v in spec.SIZES.items()}
                                | spec.EXTRA_LABELS).items():
        # REST API rather than `gh label`, which old gh versions lack.
        try:
            gh([f"repos/{REPO}/labels"], {"name": name, "color": color, "description": desc})
        except RuntimeError:  # already exists
            gh(["-X", "PATCH", f"repos/{REPO}/labels/{name.replace(' ', '%20')}"], {"color": color, "description": desc})
    print("labels ok")

    for key, (title, desc) in spec.MILESTONES.items():
        if key not in state["milestones"]:
            m = gh([f"repos/{REPO}/milestones"], {"title": title, "description": desc})
            state["milestones"][key] = m["number"]
            STATE.write_text(json.dumps(state, indent=2))
    print("milestones ok")

    numbers = {k: v["number"] for k, v in state["issues"].items()}
    for i in spec.ISSUES:
        if i["key"] in state["issues"]:
            continue
        r = gh([f"repos/{REPO}/issues"], payload_for(i, numbers, state))
        state["issues"][i["key"]] = {"number": r["number"], "id": r["id"]}
        numbers[i["key"]] = r["number"]
        STATE.write_text(json.dumps(state, indent=2))
        print(f"#{r['number']} {i['title']}")
        time.sleep(1.5)  # stay under GitHub's content-creation rate limits

    for key in UPDATE:
        assert key in state["issues"], f"--update: {key} has not been created yet"
        gh(["-X", "PATCH", f"repos/{REPO}/issues/{numbers[key]}"], payload_for(by_key[key], numbers, state))
        print(f"updated #{numbers[key]} {by_key[key]['title']}")
        time.sleep(1)

    # Native "blocked by" relationships; skipped if the API isn't available.
    links = state.setdefault("links", [])
    for i in spec.ISSUES:
        for d in i["deps"]:
            pair = [i["key"], d]
            if pair in links:
                continue
            try:
                gh(["-X", "POST", f"repos/{REPO}/issues/{numbers[i['key']]}/dependencies/blocked_by",
                    "-H", "X-GitHub-Api-Version: 2022-11-28"], {"issue_id": state["issues"][d]["id"]})
            except RuntimeError as e:
                print(f"dependency API unavailable, keeping text links only: {e}")
                STATE.write_text(json.dumps(state, indent=2))
                return
            links.append(pair)
            STATE.write_text(json.dumps(state, indent=2))
            time.sleep(0.5)
    print(f"dependencies ok: {len(links)}")


main()
