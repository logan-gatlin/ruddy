import { build } from 'esbuild';
import { downloadAndUnzipVSCode } from '@vscode/test-electron';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
// Agent hosts may themselves run inside Electron's Node mode.
delete process.env.ELECTRON_RUN_AS_NODE;
const temporary = await mkdtemp(path.join(tmpdir(), 'ruddy-vscode-'));
const binary = process.env.RUDDY_TEST_BINARY ?? path.resolve(root, '../../target/debug', process.platform === 'win32' ? 'ruddy.exe' : 'ruddy');
let passed = false;
try {
  const projects = ['first [project]', 'second', 'first [project]/nested'];
  for (const [index, name] of projects.entries()) {
    const directory = path.join(temporary, name);
    await mkdir(directory, { recursive: true });
    await writeFile(path.join(directory, 'Ruddy.toml'), `name = "test${index}"\nversion = "0.1.0"\nkind = "library"\nroot = "lib.rud"\n[dependencies]\nstd = false\n`);
    await writeFile(path.join(directory, 'lib.rud'), `let value = ${['1n', 'true', '"nested"'][index]}\nlet result = value\n`);
  }
  await mkdir(path.join(temporary, 'standalone'));
  await writeFile(path.join(temporary, 'standalone/file.rud'), 'let alone = #Some 42n\n');
  const workspaceFile = path.join(temporary, 'test.code-workspace');
  await writeFile(workspaceFile, JSON.stringify({
    folders: [...projects, 'standalone'].map(name => ({ path: name })),
    settings: { 'ruddy.server.path': binary },
  }));
  await build({ absWorkingDir: root, entryPoints: ['test/vscode.test.ts'], outfile: 'out/vscode.test.cjs', bundle: true, platform: 'node', format: 'cjs', target: 'node20', external: ['vscode'] });
  const untrusted = process.argv.includes('--untrusted');
  const userData = path.join(temporary, 'user-data');
  await mkdir(path.join(userData, 'User'), { recursive: true });
  await writeFile(path.join(userData, 'User/settings.json'), JSON.stringify({
    'security.workspace.trust.startupPrompt': 'never',
    'workbench.startupEditor': 'none', 'window.restoreWindows': 'none',
  }));
  // runTests always adds --disable-workspace-trust; launch directly so the
  // Restricted Mode test exercises VS Code's real trust boundary as well.
  const executable = await downloadAndUnzipVSCode(process.env.VSCODE_TEST_VERSION ?? '1.96.4');
  await new Promise((resolve, reject) => {
    const child = spawn(executable, [
      workspaceFile, '--user-data-dir', userData, '--extensions-dir', path.join(temporary, 'extensions'),
      '--disable-extensions', '--skip-welcome', '--skip-release-notes', '--no-sandbox', '--disable-gpu', '--disable-updates',
      `--extensionDevelopmentPath=${process.env.RUDDY_TEST_EXTENSION_PATH ?? root}`, `--extensionTestsPath=${path.join(root, 'out/vscode.test.cjs')}`,
      ...(untrusted ? [] : ['--disable-workspace-trust']),
    ], { stdio: 'inherit', env: { ...process.env, RUDDY_TEST_ROOT: temporary, RUDDY_TEST_UNTRUSTED: untrusted ? '1' : '0' } });
    const timeout = setTimeout(() => { child.kill(); reject(new Error('VS Code tests timed out')); }, 120_000);
    child.on('error', error => { clearTimeout(timeout); reject(error); });
    child.on('exit', code => { clearTimeout(timeout); if (code === 0) resolve(); else reject(new Error(`VS Code tests exited with ${code}`)); });
  });
  passed = true;
} finally {
  if (passed) await rm(temporary, { recursive: true, force: true });
  else console.error(`Test workspace and logs retained at ${temporary}`);
}
