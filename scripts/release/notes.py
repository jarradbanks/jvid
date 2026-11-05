"""Render reviewed highlights with the shared, minimal release download footer."""
import re
from pathlib import Path


def render(root, version, highlights, repository='jarradbanks/jvid', *, action='', sources_url=None):
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version):
        raise ValueError('Expected a stable release version')
    base = 'https://github.com/' + repository
    footer = (Path(root) / 'packaging/release-notes-template.md').read_text(encoding='utf-8')
    for key, value in {'VERSION': version, 'REPOSITORY': base,
                       'DOWNLOADS': base + '/releases/download/v' + version,
                       'SOURCES': sources_url or base + '/releases/tag/sources-v' + version}.items():
        footer = footer.replace('@' + key + '@', value)
    parts = ['\n'.join('- ' + line.strip() for line in highlights if line.strip())]
    if action.strip():
        parts.append(action.strip())
    parts.append(footer.strip())
    return '\n\n'.join(part for part in parts if part) + '\n'


def draft_highlights(subjects):
    """Commit subjects are a draft; the release-notes skill performs editorial review."""
    result = []
    for subject in subjects:
        if re.match(r'^(test|tests|ci|build|chore|docs|style)(\([^)]*\))?!?:', subject, re.I):
            continue
        if re.search(r'\b(snapshot|snapshots|fixture|fixtures)\b', subject, re.I):
            continue
        subject = re.sub(r'^(feat|fix|perf|refactor)(\([^)]*\))?!?:\s*', '', subject, flags=re.I)
        subject = re.sub(r'\s*\(#\d+\)$', '', subject).strip()
        if subject and subject not in result:
            result.append(subject[0].upper() + subject[1:])
    return result
