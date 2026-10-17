"""Generate docs/ROADMAP.md from spec.py, created.json and live issue status from gh.

usage: python3 roadmap.py <output path>
"""
import json
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import spec  # noqa: E402

REPO = "jarradbanks/jvid"
REPO_URL = f"https://github.com/{REPO}"
DAYS = {"S": 1, "M": 3, "L": 8}  # rough person-days per size label


def statuses(issues, prs):
    """Map issue number to Status, from `gh issue list` and `gh pr list` JSON. Issues with no status are left out."""
    in_pr = set()
    for pr in prs:
        in_pr.update(ref["number"] for ref in pr.get("closingIssuesReferences") or [])
        in_pr.update(int(n) for n in re.findall(r"#(\d+)\b", pr.get("body") or ""))
    out = {}
    for i in issues:
        if i["state"] == "CLOSED":
            out[i["number"]] = "done"
        elif any(label["name"] == "need testing" for label in i["labels"]):
            out[i["number"]] = "need testing"
        elif i["number"] in in_pr:
            out[i["number"]] = "in progress"
    return out


def gh_json(*args):
    run = subprocess.run(["gh", *args], capture_output=True, text=True, check=True, timeout=60)
    return json.loads(run.stdout)


def fetch_statuses():
    """Live status per issue number, or {} with a warning when gh is missing or fails."""
    try:
        issues = gh_json("issue", "list", "-R", REPO, "--state", "all", "--limit", "1000",
                         "--json", "number,state,labels")
        prs = gh_json("pr", "list", "-R", REPO, "--state", "open", "--limit", "1000",
                      "--json", "body,closingIssuesReferences")
    except (OSError, subprocess.SubprocessError, ValueError) as e:
        err = e.stderr.strip() if isinstance(e, subprocess.CalledProcessError) and e.stderr else ""
        why = err.splitlines()[0] if err else e
        print(f"warning: could not read issue status from gh ({why}); Status column left blank", file=sys.stderr)
        return {}
    return statuses(issues, prs)

state = json.loads((pathlib.Path(__file__).parent / "created.json").read_text())
num = {k: v["number"] for k, v in state["issues"].items()}
by_key = {i["key"]: i for i in spec.ISSUES}
planned = [i for i in spec.ISSUES if i["ms"]]


def link(key):
    return f"[#{num[key]}]({REPO_URL}/issues/{num[key]})"


def ms_link(ms):
    return f"[{spec.MILESTONES[ms][0]}]({REPO_URL}/milestone/{state['milestones'][ms]})"


def days(issues):
    return sum(DAYS[i["size"]] for i in issues)


# Longest path through the dependency graph, weighted by size.
best = {}
for i in planned:
    prev = max(i["deps"], key=lambda d: best[d][0], default=None)
    best[i["key"]] = (DAYS[i["size"]] + (best[prev][0] if prev else 0), prev)
end = max(best, key=lambda k: best[k][0])
critical, k = [], end
while k:
    critical.append(k)
    k = best[k][1]
critical.reverse()


def main(path):
    status = fetch_statuses()
    out = []
    w = out.append

    w("# jvid roadmap\n")
    w("The order jvid gets built in. Every item is a GitHub issue with scope, acceptance criteria and dependencies. "
      "The feature set and architecture behind them are in [PLAN.md](PLAN.md).\n")
    w("**Picking up work:** take the first open issue in the current milestone's table whose dependencies are all closed. The tables below are in build order, so issue numbers don't always follow it. "
      "Issues tagged [`good first issue`](" + REPO_URL + "/labels/good%20first%20issue) are self-contained.\n")

    w("## Releases\n")
    w("| Release | Milestones | What you can do |")
    w("| --- | --- | --- |")
    for name, mss, what in spec.RELEASES:
        w(f"| **{name}** | {', '.join(ms_link(m) for m in mss)} | {what} |")
    w("")

    w("## How the order was chosen\n")
    w("1. **Biggest risks first.** FFmpeg linking on three platforms and frame-accurate decoding of long, "
      "variable-frame-rate recordings are the parts most likely to go wrong, so they land in M0 and M1.")
    w("2. **Model before UI.** Time, the project model and undo/redo exist before anything edits a project, "
      "so every feature is undoable and saveable from its first commit.")
    w("3. **Playable early.** By the end of M2 you can cut a multi-layer timeline and play it back. "
      "M3 adds animation; M4 makes it exportable and ships v0.1.0.")
    w("4. **One evaluation path.** Playback, export and the CLI all use the same timeline evaluation "
      f"({link('eval')}), so what you see is what you export.")
    w("5. **Polish after usefulness.** Titles, transitions, color and proxies wait for v0.2.0; they build on the "
      "transform and keyframe work rather than shaping it.\n")

    w("## Critical path\n")
    w(f"The longest chain of dependent issues, about **{best[end][0]} person-days**. "
      "Everything else can happen alongside it.\n")
    w(" → ".join(f"{link(k)} {by_key[k]['title']}" for k in critical) + "\n")

    w("## Parallel tracks\n")
    w("After M0, work splits into tracks that rarely block each other:\n")
    w("| Track | Crates | Issues |")
    w("| --- | --- | --- |")
    tracks = [
        ("Core", "`jvid-core`", lambda i: "core" in i["areas"]),
        ("Media", "`jvid-media`", lambda i: "media" in i["areas"]),
        ("Render", "`jvid-render`", lambda i: "render" in i["areas"]),
        ("Audio", "`jvid-audio`", lambda i: "audio" in i["areas"]),
        ("UI and timeline", "`jvid-app`, `suite-ui`", lambda i: {"ui", "timeline", "suite-ui"} & set(i["areas"])),
        ("Export and CLI", "`jvid-media`, `jvid-cli`", lambda i: {"export", "cli"} & set(i["areas"])),
    ]
    for name, crates, pred in tracks:
        w(f"| {name} | {crates} | {' '.join(link(i['key']) for i in planned if pred(i))} |")
    w("")

    w("## Milestones\n")
    w("Sizes: **S** up to a day, **M** two to four days, **L** one to two weeks. "
      "Person-day totals are rough, for one developer.\n")
    for ms, (title, desc) in spec.MILESTONES.items():
        items = [i for i in planned if i["ms"] == ms]
        w(f"### {ms_link(ms)}\n")
        w(f"{desc} {len(items)} issues, about {days(items)} person-days.\n")
        keys = {i["key"] for i in items}
        w("```mermaid")
        w("flowchart LR")
        for i in items:
            w(f"  {i['key'].replace('-', '_')}[\"#35;{num[i['key']]} {i['title']}\"]")
        for i in items:
            for d in i["deps"]:
                if d in keys:
                    w(f"  {d.replace('-', '_')} --> {i['key'].replace('-', '_')}")
        w("```\n")
        w("| Issue | Title | Status | Size | Area | Depends on |")
        w("| --- | --- | --- | --- | --- | --- |")
        for i in items:
            deps = ", ".join(link(d) for d in i["deps"]) or "—"
            w(f"| {link(i['key'])} | {i['title']} | {status.get(num[i['key']], '')} | {i['size']} "
              f"| {', '.join(i['areas'])} | {deps} |")
        w("")

    w("## Later\n")
    w("Planned after v0.3.0, or better suited to plugins. Tracked with the "
      f"[`later`]({REPO_URL}/labels/later) label.\n")
    for i in spec.ISSUES:
        if i["ms"] is None:
            w(f"- {link(i['key'])} **{i['title']}** — {i['summary']}")
    w("")

    w("---\n")
    w("Generated by `scripts/roadmap/roadmap.py` from `scripts/roadmap/spec.py`. To add work, add an entry to the spec, "
      "run `create.py` (it skips issues that already exist), then rerun `roadmap.py docs/ROADMAP.md`.")
    pathlib.Path(path).write_text("\n".join(out) + "\n")
    print(f"wrote {path}; critical path {best[end][0]} days over {len(critical)} issues; total {days(planned)} days")


if __name__ == "__main__":
    main(sys.argv[1])
