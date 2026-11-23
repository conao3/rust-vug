const fs = require('node:fs');
const path = require('node:path');

const [prefix, node] = process.argv.slice(1);
const manifest = JSON.parse(fs.readFileSync('package.json', 'utf8'));
const name = /^(?:@[a-z0-9][a-z0-9._-]*\/)?([a-z0-9][a-z0-9._-]*)$/.exec(manifest.name);
if (!name) throw new Error('package.json: invalid package name');
const modules = path.join(prefix, 'lib/node_modules');
const destination = path.join(modules, manifest.name);
const scripts = typeof manifest.bin === 'string' ? { [name[1]]: manifest.bin } : manifest.bin ?? {};
const root = fs.realpathSync('.');
const binDir = path.join(prefix, 'bin');
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";

for (const [bin, target] of Object.entries(scripts)) {
    if (!/^[a-zA-Z0-9][a-zA-Z0-9._-]*$/.test(bin) || typeof target !== 'string'
        || path.isAbsolute(target) || target.split('/').includes('..')) {
        throw new Error('package.json: invalid bin name or path');
    }
    const source = fs.realpathSync(target);
    if (!source.startsWith(root + '/') || !fs.statSync(source).isFile()) {
        throw new Error('package.json: bin target must be a file within the package');
    }
    fs.mkdirSync(binDir, { recursive: true });
    fs.writeFileSync(path.join(binDir, bin),
        '#!/bin/sh\nexec ' + quote(node) + ' ' + quote(path.join(destination, target)) + ' "$@"\n',
        { flag: 'wx', mode: 0o755 });
}

fs.mkdirSync(path.dirname(destination), { recursive: true });
fs.cpSync(root, destination, { recursive: true, verbatimSymlinks: true, force: false, errorOnExist: true });
