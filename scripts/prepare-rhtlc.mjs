// Build-time only: pin and verify the RHTLC transport, never the Python/GUI client.
import { createHash } from 'node:crypto';
import { chmod, mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';

const directory = fileURLToPath(new URL('../src-tauri/binaries/', import.meta.url));
const release = 'https://github.com/tmichett/wstunnel/releases/download/v6.0.1/';
const assets = {
  'aarch64-apple-darwin': ['rhtlc-wstunnel-macos-arm64', '676702bf3fa42373c8b5d33d78a5977be1b3d6f8429d3de8683b19411f53e214'],
  'x86_64-apple-darwin': ['rhtlc-wstunnel-macos-amd64', 'd1ddbd53b3b415662d6ad2c71f9572d278fdd12c29b5b75f75947c0debdd5824'],
  'x86_64-pc-windows-msvc': ['rhtlc-wstunnel-windows-amd64.exe', '595aa1665daebdc3f76857524c2c04af1c331aead040730309e61d5e40d9e2c2'],
};
const host = process.platform === 'darwin'
  ? `${process.arch === 'arm64' ? 'aarch64' : 'x86_64'}-apple-darwin`
  : process.platform === 'win32' && process.arch === 'x64' ? 'x86_64-pc-windows-msvc' : '';
const target = process.argv[2] || process.env.TAURI_ENV_TARGET_TRIPLE || host;
const digest = (data) => createHash('sha256').update(data).digest('hex');
const filename = (triple) => join(directory, `rhtlc-wstunnel-${triple}${triple.includes('windows') ? '.exe' : ''}`);

async function prepare(triple) {
  const asset = assets[triple];
  if (!asset) throw new Error(`Unsupported RHTLC build target: ${triple}`);
  const [name, sha256] = asset;
  const path = filename(triple);
  const cached = await readFile(path).catch(() => null);
  if (!cached || digest(cached) !== sha256) {
    console.log(`Preparing RHTLC transport 6.0.1 for ${triple}`);
    const response = await fetch(`${release}${name}`, { signal: AbortSignal.timeout(120_000) });
    if (!response.ok) throw new Error(`RHTLC download failed: HTTP ${response.status}`);
    const data = Buffer.from(await response.arrayBuffer());
    if (digest(data) !== sha256) throw new Error(`RHTLC checksum mismatch: ${name}`);
    const temporary = `${path}.${process.pid}.tmp`;
    await writeFile(temporary, data, { mode: 0o755 });
    await rename(temporary, path);
  }
  if (process.platform !== 'win32') await chmod(path, 0o755);
}

await mkdir(directory, { recursive: true });
if (target === 'universal-apple-darwin') {
  if (process.platform !== 'darwin') throw new Error('Universal macOS builds require macOS and lipo.');
  await Promise.all(['aarch64-apple-darwin', 'x86_64-apple-darwin'].map(prepare));
  execFileSync('lipo', ['-create', filename('aarch64-apple-darwin'), filename('x86_64-apple-darwin'), '-output', filename(target)]);
  await chmod(filename(target), 0o755);
} else {
  await prepare(target);
}
console.log(`RHTLC transport ready: ${target}`);
