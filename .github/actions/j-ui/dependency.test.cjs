const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { test } = require('node:test');
const { pathToFileURL } = require('node:url');
const { lockedDependency, configureMirror, REPOSITORY } = require('./dependency.cjs');

const revision = '0123456789abcdef0123456789abcdef01234567';
const lock = `[[package]]\nname = "j-ui"\nversion = "0.1.0"\nsource = "git+${REPOSITORY}?tag=v0.1.0#${revision}"\n`;

test('uses the lockfile tag and commit, including Windows line endings', () => {
    for (const text of [lock, lock.replaceAll('\n', '\r\n')]) {
        assert.deepEqual(lockedDependency(text), { tag: 'v0.1.0', revision });
    }
    for (const text of [lock + lock, lock.replace(REPOSITORY, 'https://github.com/other/j-ui'),
        lock.replace('?tag=v0.1.0', '?branch=main'), lock.replace('v0.1.0', 'v0.1.0%0Ainjected=value')]) {
        assert.throws(() => lockedDependency(text));
    }
});

test('rejects a moved tag and fetches only j-ui through a credential-free local mirror', t => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'j-ui-auth-'));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const mirror = path.join(root, 'checkout with spaces');
    const env = { ...process.env, GIT_CONFIG_GLOBAL: path.join(root, 'gitconfig'), GIT_CONFIG_NOSYSTEM: '1' };
    const git = args => execFileSync('git', args, { env, encoding: 'utf8' }).trim();
    git(['init', '--quiet', mirror]);
    git(['-C', mirror, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
        'commit', '--quiet', '--allow-empty', '-m', 'fixture']);
    const head = git(['-C', mirror, 'rev-parse', 'HEAD']);
    git(['-C', mirror, 'tag', 'v0.1.0']);
    assert.throws(() => configureMirror(mirror, revision, env), /differs from Cargo.lock/);
    assert.equal(fs.existsSync(env.GIT_CONFIG_GLOBAL), false);
    configureMirror(mirror, head, env);
    const url = pathToFileURL(mirror).href;
    assert.equal(git(['ls-remote', '--get-url', REPOSITORY]), url);
    assert.equal(git(['ls-remote', '--get-url', `${REPOSITORY}.git`]), url);
    assert.equal(git(['ls-remote', '--get-url', 'https://github.com/jarradbanks/jvid']),
        'https://github.com/jarradbanks/jvid');
    assert.equal(git(['ls-remote', REPOSITORY, 'refs/tags/v0.1.0']).split(/\s+/)[0], head);
});
