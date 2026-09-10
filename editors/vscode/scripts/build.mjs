import { build } from 'esbuild';
import { execFileSync } from 'node:child_process';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = fileURLToPath(new URL('..', import.meta.url));
const grammar = path.resolve(root, '../../treesitter');
const dist = path.join(root, 'dist');
await mkdir(dist, { recursive: true });
const cli = path.join(root, 'node_modules/tree-sitter-cli/cli.js');
execFileSync(process.execPath, [cli, 'build', '--wasm', '--output', path.join(dist, 'tree-sitter-ruddy.wasm'), grammar], { cwd: root, stdio: 'inherit' });
await copyFile(path.join(root, 'node_modules/web-tree-sitter/web-tree-sitter.wasm'), path.join(dist, 'web-tree-sitter.wasm'));
await copyFile(path.join(grammar, 'queries/highlights.scm'), path.join(dist, 'highlights.scm'));
const result = await build({
  absWorkingDir: root, entryPoints: ['src/extension.ts'], outfile: 'dist/extension.cjs',
  bundle: true, platform: 'node', format: 'cjs', target: 'node20',
  // The ESM entry uses import.meta.url, which cannot survive a CJS bundle.
  alias: { 'web-tree-sitter': path.join(root, 'node_modules/web-tree-sitter/web-tree-sitter.cjs') },
  external: ['vscode'], metafile: true,
});
// Retain the licenses of every dependency included in the JavaScript bundle.
const packages = new Set(Object.keys(result.metafile.inputs).flatMap(input => {
  const match = input.match(/node_modules\/((?:@[^/]+\/)?[^/]+)/);
  return match ? [match[1]] : [];
}));
const notices = [];
for (const name of [...packages].sort()) {
  const directory = path.join(root, 'node_modules', name);
  let license;
  for (const filename of ['LICENSE', 'LICENSE.txt', 'License.txt', 'LICENSE.md']) {
    try { license = await readFile(path.join(directory, filename), 'utf8'); break; }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  if (!license) throw new Error(`Missing license for bundled package ${name}`);
  notices.push(`${name}\n${license}`);
}
await writeFile(path.join(dist, 'THIRD_PARTY_LICENSES.txt'), notices.join('\n\n'));
