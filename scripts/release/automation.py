#!/usr/bin/env python3
"""Prepare reviewed release branches and explicitly dispatch approved tag builds."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.parse

try:
    import tomllib
except ModuleNotFoundError:  # Local Python 3.10 tooling; CI uses Python 3.12.
    import tomli as tomllib

from github_api import github, pages, release_for_tag, tag_commit

PLAN = '.github/release-plan.json'
RELEASE_FILES = {'Cargo.toml', 'Cargo.lock', 'packaging/release-notes.md', PLAN}
SHA = re.compile(r'[0-9a-f]{40}\Z')
VERSION = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z')


def run(*args, cwd=None):
    return subprocess.check_output([str(x) for x in args], cwd=cwd, text=True).strip()


def version(root):
    return tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']


def bump(current, kind):
    match = VERSION.fullmatch(current)
    if not match or kind not in ('patch', 'minor', 'major'):
        raise ValueError('Choose patch/minor/major for a stable major.minor.patch version')
    a, b, c = map(int, match.groups())
    return {'patch': f'{a}.{b}.{c + 1}', 'minor': f'{a}.{b + 1}.0', 'major': f'{a + 1}.0.0'}[kind]


def version_files(root, new):
    """Change workspace package versions without upgrading any registry dependency."""
    old = version(root)
    if not VERSION.fullmatch(new) or tuple(map(int, new.split('.'))) <= tuple(map(int, old.split('.'))):
        raise ValueError('The release version must increase')
    manifest = root / 'Cargo.toml'
    data = tomllib.loads(manifest.read_text())
    names = set()
    for pattern in data['workspace']['members']:
        for member in root.glob(pattern):
            path = member / 'Cargo.toml'
            if not path.is_file():
                continue
            package = tomllib.loads(path.read_text()).get('package', {})
            if package.get('version') == {'workspace': True}:
                names.add(package['name'])
    if not names:
        raise ValueError('No workspace-versioned packages found')
    text = manifest.read_bytes().decode('utf-8')
    start = text.index('[workspace.package]')
    end = text.find('\n[', start + 1)
    end = len(text) if end < 0 else end
    section, count = re.subn(r'(?m)^(\s*version\s*=\s*)"' + re.escape(old) + '"',
                             lambda m: m[1] + json.dumps(new), text[start:end])
    if count != 1:
        raise ValueError('Expected one workspace version')
    lock = root / 'Cargo.lock'
    original = lock.read_bytes().decode('utf-8')
    parts = original.split('[[package]]')
    found = set()
    for index, block in enumerate(parts[1:], 1):
        record = tomllib.loads('[[package]]' + block)['package'][0]
        if record['name'] in names and 'source' not in record:
            if record['version'] != old or record['name'] in found:
                raise ValueError('Workspace lock version differs from manifest')
            found.add(record['name'])
            block, count = re.subn(r'(?m)^version = "' + re.escape(old) + '"(?=\r?$)',
                                   'version = ' + json.dumps(new), block)
            if count != 1:
                raise ValueError('Invalid workspace lock entry')
            for name in names:
                block = block.replace(json.dumps(name + ' ' + old), json.dumps(name + ' ' + new))
        parts[index] = block
    if found != names:
        raise ValueError('Missing workspace lock entries')
    changed = '[[package]]'.join(parts)
    # Registry and Git package identity/checksum records must remain untouched.
    external = lambda value: [p for p in tomllib.loads(value)['package'] if 'source' in p]
    if external(changed) != external(original):
        raise ValueError('Release preparation changed external dependency records')
    manifest.write_bytes((text[:start] + section + text[end:]).encode('utf-8'))
    lock.write_bytes(changed.encode('utf-8'))


def output(**values):
    path = os.environ.get('GITHUB_OUTPUT')
    if path:
        with open(path, 'a', encoding='utf-8') as stream:
            for key, value in values.items():
                stream.write(f'{key}={value}\n')
    print(json.dumps(values))


def version_at(root, ref):
    return tomllib.loads(run('git', 'show', ref + ':Cargo.toml', cwd=root))['workspace']['package']['version']


def stable(value):
    if not isinstance(value, str) or not VERSION.fullmatch(value):
        raise ValueError('Expected a canonical stable major.minor.patch version')
    return tuple(map(int, value.split('.')))


def read_plan(root):
    plan = json.loads((root / PLAN).read_text())
    fields = {'schema', 'version', 'previous_version', 'bump', 'source_branch',
              'source_commit', 'main_commit', 'base_commit', 'notes_sha256'}
    if (set(plan) != fields or type(plan['schema']) is not int or plan['schema'] != 1
            or plan['source_branch'] != 'develop'
            or any(not isinstance(plan[k], str) or not SHA.fullmatch(plan[k])
                   for k in ('source_commit', 'main_commit', 'base_commit'))
            or plan['version'] != version(root)
            or plan['version'] != bump(plan['previous_version'], plan['bump'])):
        raise ValueError('Invalid release plan or workspace version')
    notes = (root / 'packaging/release-notes.md').read_bytes()
    if not notes.strip() or hashlib.sha256(notes).hexdigest() != plan['notes_sha256']:
        raise ValueError('Release notes differ from the frozen candidate plan')
    base = plan['base_commit']
    if version_at(root, base) != plan['previous_version']:
        raise ValueError('Release base version does not match the plan')
    parents = run('git', 'rev-list', '--parents', '-n', '1', base, cwd=root).split()[1:]
    if base != plan['source_commit'] and parents != [plan['source_commit'], plan['main_commit']]:
        raise ValueError('Release base must be the frozen develop snapshot or its exact merge with main')
    for ancestor in ('source_commit', 'main_commit'):
        run('git', 'merge-base', '--is-ancestor', plan[ancestor], base, cwd=root)
    run('git', 'merge-base', '--is-ancestor', base, 'HEAD', cwd=root)
    changed = set(run('git', 'diff', '--name-only', base, 'HEAD', cwd=root).splitlines())
    if not changed <= RELEASE_FILES:
        raise ValueError('Frozen candidate contains unplanned code changes')
    with tempfile.TemporaryDirectory() as temporary:
        snapshot = Path(temporary)
        paths = run('git', 'ls-tree', '-r', '--name-only', base, cwd=root).splitlines()
        for path in paths:
            if path == 'Cargo.lock' or path.endswith('Cargo.toml'):
                destination = snapshot / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(subprocess.check_output(['git', 'show', base + ':' + path], cwd=root))
        version_files(snapshot, plan['version'])
        for name in ('Cargo.toml', 'Cargo.lock'):
            if (snapshot / name).read_bytes() != (root / name).read_bytes():
                raise ValueError('Version/lock edits differ from the frozen release plan')
    return plan


def workflow_pending(repository, workflow, commit, title=None):
    runs = pages(f'repos/{repository}/actions/workflows/{workflow}/runs?event=workflow_dispatch',
                 key='workflow_runs', api=github)
    return any(r['head_sha'] == commit and (title is None or r['display_title'] == title)
               and (r['status'] != 'completed' or r['conclusion'] == 'success') for r in runs)


def prepare_candidate(root, kind, repository):
    if run('git', 'status', '--porcelain', cwd=root):
        raise RuntimeError('Release preparation requires a clean checkout')
    source = run('git', 'rev-parse', 'HEAD', cwd=root)
    if source != run('git', 'rev-parse', 'origin/develop', cwd=root):
        raise ValueError('Prepare must start at the fetched develop snapshot')
    main = run('git', 'rev-parse', 'origin/main', cwd=root)
    current = max((version(root), version_at(root, main)), key=stable)
    new = bump(current, kind)
    branch = 'release/v' + new
    prs = list(pages(f'repos/{repository}/pulls?state=all&base=main', api=github))
    if any(p['state'] == 'open' and p['head']['ref'].startswith('release/')
           and (p['head']['ref'] != branch or not p['head'].get('repo')
                or p['head']['repo']['full_name'] != repository) for p in prs):
        raise RuntimeError('An existing release PR must finish first')
    matching = [p for p in prs if p['head']['ref'] == branch
                and p['head']['repo'] and p['head']['repo']['full_name'] == repository]
    if any(p.get('merged_at') for p in matching):
        raise RuntimeError('Release candidate version was already merged; do not reuse it')
    closed = [p for p in matching if p['state'] != 'open']
    matching = [p for p in matching if p['state'] == 'open']
    if len(matching) > 1:
        raise RuntimeError('Duplicate open release candidate PRs')
    if tag_commit(repository, 'v' + new, api=github) or release_for_tag(repository, 'v' + new, api=github):
        raise RuntimeError('This release version already has a tag or release')
    reference = github(f'repos/{repository}/git/ref/heads/' + urllib.parse.quote(branch, safe=''))
    if reference:
        if closed and not matching:
            raise RuntimeError('Closed candidate branch must be explicitly abandoned before replacement')
        frozen = reference['object']['sha']
        run('git', 'fetch', 'origin', 'refs/heads/' + branch, cwd=root)
        if run('git', 'rev-parse', 'FETCH_HEAD', cwd=root) != frozen:
            raise RuntimeError('Candidate branch changed during preparation; retry')
        run('git', 'switch', '--detach', frozen, cwd=root)
        plan = read_plan(root)
        if plan['version'] != new or plan['main_commit'] != main or plan['bump'] != kind:
            raise RuntimeError('Existing candidate conflicts with requested version or main base')
    else:
        if matching:
            raise RuntimeError('Release PR exists but its frozen branch is missing')
        if run('git', 'diff', '--name-only', main, source, cwd=root) == '':
            raise RuntimeError('Develop has no new changes to release')
        run('git', 'switch', '-c', branch, cwd=root)
        run('git', 'merge', '--no-edit', main, cwd=root)
        base = run('git', 'rev-parse', 'HEAD', cwd=root)
        if run('git', 'diff', '--name-only', main, base, cwd=root) == '':
            raise RuntimeError('Develop has no new changes after including main')
        if version(root) != current:
            raise RuntimeError('Merged workspace version differs from selected release base')
        version_files(root, new)
        subjects = run('git', 'log', '--no-merges', '--format=%s', main + '..' + source, cwd=root).splitlines()
        template = (root / 'packaging/release-notes-template.md').read_text()
        notes = ('Changes in ' + new + '\n\n' + '\n'.join('- ' + s for s in subjects) + '\n\n' + template)
        (root / 'packaging/release-notes.md').write_text(notes)
        (root / PLAN).parent.mkdir(parents=True, exist_ok=True)
        (root / PLAN).write_text(json.dumps({'schema': 1, 'version': new, 'previous_version': current,
            'bump': kind, 'source_commit': source, 'source_branch': 'develop', 'main_commit': main,
            'base_commit': base, 'notes_sha256': hashlib.sha256(notes.encode()).hexdigest()}, indent=2) + '\n')
        run('git', 'add', *sorted(RELEASE_FILES), cwd=root)
        run('git', '-c', 'gc.auto=0', '-c', 'maintenance.auto=false', 'commit', '-m', 'Prepare jvid ' + new, cwd=root)
        read_plan(root)
        run('git', 'push', 'origin', 'HEAD:refs/heads/' + branch, cwd=root)
    frozen = run('git', 'rev-parse', 'HEAD', cwd=root)
    if matching:
        pr = matching[0]
    else:
        body = ('Frozen release candidate from develop, with version/lock edits and generated notes.\n\n'
                'Review CI and all three Package candidates plus source and clean-machine evidence. '
                'Merge into main when ready; the immutable merged tag build will wait for the existing '
                'release environment approval, then sign, verify uploaded assets and publish automatically. '
                'Rerunning Prepare resumes this frozen branch without adding develop commits.\n')
        pr = github(f'repos/{repository}/pulls', 'POST', {'title': 'Release jvid ' + new,
            'head': branch, 'base': 'main', 'body': body})
    for workflow in ('ci.yml', 'package.yml'):
        if not workflow_pending(repository, workflow, frozen):
            github(f'repos/{repository}/actions/workflows/{workflow}/dispatches', 'POST', {'ref': branch})
    output(version=new, branch=branch, pr_url=pr['html_url'])


def dispatch(root, repository, commit, before):
    if run('git', 'rev-parse', 'HEAD', cwd=root) != commit:
        raise ValueError('Checkout does not match the merged main commit')
    current = version(root)
    previous = version_at(root, before)
    if stable(current) <= stable(previous):
        output(release='false', reason='No workspace version increase in this main change')
        return
    if run('git', 'rev-parse', commit + '^1', cwd=root) != before:
        raise ValueError('Release must be the exact first-parent main transition, not a multi-commit range')
    if not (root / PLAN).is_file():
        raise ValueError('Version increase requires a valid frozen release plan')
    plan = read_plan(root)
    if plan['main_commit'] != before or plan['previous_version'] != previous:
        raise ValueError('Release candidate is stale relative to previous main; prepare a reviewed replacement')
    tag = 'v' + current
    target = tag_commit(repository, tag, api=github)
    if target and target != commit:
        raise ValueError('Existing release tag belongs to a different commit; refusing to retag')
    release = release_for_tag(repository, tag, api=github)
    if release:
        if not target:
            raise ValueError('Release exists without its immutable tag; refusing to recreate it')
        output(release='false', reason='Release already published' if not release['draft']
               else 'Draft exists; rerun the original Package failed jobs to preserve its assets')
        return
    if not target:
        annotation = github(f'repos/{repository}/git/tags', 'POST',
            {'tag': tag, 'message': 'jvid ' + current, 'object': commit, 'type': 'commit'})
        github(f'repos/{repository}/git/refs', 'POST', {'ref': 'refs/tags/' + tag, 'sha': annotation['sha']})
    if workflow_pending(repository, 'package.yml', commit, 'Release ' + tag):
        output(release='false', reason='Publishing build already queued or completed')
        return
    if tag_commit(repository, tag, api=github) != commit:
        raise ValueError('Release tag changed before dispatch')
    github(f'repos/{repository}/actions/workflows/package.yml/dispatches', 'POST',
        {'ref': tag, 'inputs': {'release': 'true'}})
    output(release='true', tag=tag, commit=commit)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'dispatch'])
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--repository', default=os.environ.get('GITHUB_REPOSITORY', 'jarradbanks/jvid'))
    parser.add_argument('--bump', choices=['patch', 'minor', 'major'], default='minor')
    parser.add_argument('--commit', default=os.environ.get('GITHUB_SHA'))
    parser.add_argument('--before')
    args = parser.parse_args()
    if args.command == 'prepare':
        prepare_candidate(args.root, args.bump, args.repository)
    else:
        if not args.commit or not SHA.fullmatch(args.commit):
            parser.error('--commit must identify the merged main commit')
        before = args.before or run('git', 'rev-parse', args.commit + '^1', cwd=args.root)
        if not SHA.fullmatch(before):
            parser.error('--before must identify the previous main commit')
        dispatch(args.root, args.repository, args.commit, before)


if __name__ == '__main__':
    main()
