const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const REPOSITORY = 'https://github.com/jarradbanks/j-ui';

function lockedDependency(lock) {
    const packages = lock.split(/^\[\[package\]\]\r?$/m)
        .filter(block => /^name = "j-ui"\r?$/m.test(block));
    if (packages.length !== 1) throw new Error('Expected exactly one locked j-ui package');
    const source = packages[0].match(/^source = "git\+https:\/\/github\.com\/jarradbanks\/j-ui\?tag=([^"#\r\n]+)#([0-9a-f]{40})"\r?$/m);
    if (!source) throw new Error('j-ui must use a Git tag and locked commit from jarradbanks/j-ui');
    const tag = decodeURIComponent(source[1]);
    if (!/^[A-Za-z0-9][A-Za-z0-9._/-]*$/.test(tag)) throw new Error('Invalid locked j-ui tag');
    return { tag, revision: source[2] };
}

function configureMirror(directory, expectedRevision, env = process.env) {
    if (!/^[0-9a-f]{40}$/.test(expectedRevision)) throw new Error('Invalid expected j-ui revision');
    const git = args => execFileSync('git', args, { env, encoding: 'utf8' }).trim();
    const actual = git(['-C', directory, 'rev-parse', 'HEAD']);
    if (actual !== expectedRevision) throw new Error('Fetched j-ui tag differs from Cargo.lock');
    const url = pathToFileURL(path.resolve(directory)).href;
    // Cargo keeps its original Git source identity and locked revision. Only the transport
    // changes, after checkout has authenticated and checked the tag against Cargo.lock.
    git(['config', '--global', '--replace-all', `url.${url}.insteadOf`, REPOSITORY]);
    git(['config', '--global', '--add', `url.${url}.insteadOf`, `${REPOSITORY}.git`]);
    return url;
}

if (require.main === module) {
    if (process.argv[2] === 'resolve') {
        const dependency = lockedDependency(fs.readFileSync('Cargo.lock', 'utf8'));
        fs.appendFileSync(process.env.GITHUB_OUTPUT, `tag=${dependency.tag}\nrevision=${dependency.revision}\n`);
    } else if (process.argv[2] === 'configure') {
        const directory = path.join(process.env.GITHUB_WORKSPACE, '.j-ui-ci');
        configureMirror(directory, process.env.J_UI_EXPECTED_REVISION);
        console.log('Cargo will fetch j-ui from the checkout verified against Cargo.lock.');
    } else {
        throw new Error('Expected resolve or configure');
    }
}

module.exports = { lockedDependency, configureMirror, REPOSITORY };
