"""Small GitHub API boundary shared by release automation; no credential logging."""
import json
import os
import urllib.error
import urllib.request


def github(path, method='GET', body=None):
    request = urllib.request.Request('https://api.github.com/' + path,
        data=None if body is None else json.dumps(body).encode(), method=method,
        headers={'Authorization': 'Bearer ' + os.environ['GH_TOKEN'],
                 'Accept': 'application/vnd.github+json',
                 'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'jvid-release'})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        if error.code == 404 and method == 'GET':
            return None
        raise RuntimeError(f'GitHub {method} request failed (HTTP {error.code})') from None


def pages(path, key=None, api=github):
    page = 1
    while True:
        separator = '&' if '?' in path else '?'
        response = api(f'{path}{separator}per_page=100&page={page}')
        if response is None:
            raise RuntimeError('GitHub collection is unavailable')
        records = response[key] if key else response
        yield from records
        if len(records) < 100:
            return
        page += 1


def release_for_tag(repository, tag, api=github):
    # Include drafts as well as published releases; do not assume the tag endpoint
    # returns unpublished drafts on every GitHub API version.
    release = api(f'repos/{repository}/releases/tags/{tag}')
    if release:
        return release
    matches = [r for r in pages(f'repos/{repository}/releases', api=api) if r['tag_name'] == tag]
    if len(matches) > 1:
        raise RuntimeError('Duplicate release objects for the same tag')
    return matches[0] if matches else None


def tag_commit(repository, tag, api=github):
    ref = api(f'repos/{repository}/git/ref/tags/{tag}')
    if ref is None:
        return None
    target = ref['object']
    seen = set()
    while target['type'] == 'tag':
        if target['sha'] in seen or len(seen) >= 10:
            raise RuntimeError('Invalid annotated tag chain')
        seen.add(target['sha'])
        target = api(f'repos/{repository}/git/tags/' + target['sha'])['object']
    if target['type'] != 'commit':
        raise RuntimeError('Release tag does not identify a commit')
    return target['sha']
