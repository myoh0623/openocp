import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import vm from 'node:vm';
import test from 'node:test';
import { normalizeLanguage, translate } from '../src/i18n.js';

// Run the actual UI controller with a small DOM and IPC boundary. No native app
// or lab is needed to exercise event ordering, validation, and profile storage.
async function fixture() {
  const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
  const elements = new Map();
  const classes = () => {
    const values = new Set();
    return { add: x => values.add(x), remove: x => values.delete(x), contains: x => values.has(x),
      toggle(x, on) { if (on) values.add(x); else values.delete(x); } };
  };
  for (const [, tag, id] of html.matchAll(/(<[^>]*\bid="([^"]+)"[^>]*>)/g)) {
    elements.set(id, { value: '', type: /type="checkbox"/.test(tag) ? 'checkbox' : 'text',
      checked: false, textContent: '', classList: classes(), addEventListener() {},
      replaceChildren() {}, add() {}, focus() {} });
  }
  const saved = new Map(), events = new Map(), calls = [];
  let resolveStart, resolveTool;
  const start = new Promise(resolve => { resolveStart = resolve; });
  const tool = new Promise(resolve => { resolveTool = resolve; });
  const context = vm.createContext({
    document: { getElementById: id => { assert.ok(elements.has(id), id); return elements.get(id); },
      documentElement: {}, querySelector: () => ({ classList: classes() }) },
    localStorage: { getItem: k => saved.get(k) ?? null, setItem: (k, v) => saved.set(k, v), removeItem: k => saved.delete(k) },
    window: { __TAURI_INTERNALS__: true, setTimeout() {}, clearTimeout() {} }, navigator: { userAgent: 'Mac' },
    crypto: { randomUUID }, Option: class {}, URL, Intl, appVersion: '2.2.0', normalizeLanguage, translate,
    applyTranslations() {}, listen: async (name, fn) => events.set(name, fn),
    invoke: async (name, args) => {
      calls.push({ name, args });
      if (name === 'start_connection') return start;
      if (name === 'open_external_tool') return tool;
      return { name: 'Test browser', path: '/test/browser' };
    }
  });
  // Exercise Windows checkout line endings on every runner.
  const source = readFileSync(new URL('../src/main.js', import.meta.url), 'utf8')
    .replace(/\r?\n/g, '\r\n').replace(/^import .*;\r?\n/gm, '');
  vm.runInContext(source, context);
  await new Promise(resolve => setImmediate(resolve));
  elements.get('sshIP').value = '192.0.2.10';
  return { elements, saved, events, calls, resolveStart, resolveTool, run: code => vm.runInContext(code, context) };
}

test('Connect enables tools without opening a browser, and closing browsers keeps tools active', async () => {
  const f = await fixture();
  const pending = f.run('connect()');
  f.resolveStart({ socksPort: 1080 }); await pending;
  assert.equal(f.calls.filter(c => c.name === 'open_browser_session').length, 0);
  assert.equal(f.elements.get('terminalButton').disabled, false);
  f.events.get('browser-session-count')({ payload: { count: 0 } });
  assert.equal(f.elements.get('terminalButton').disabled, false);
  f.events.get('tunnel-status')({ payload: { state: 'stopped' } });
  assert.equal(f.elements.get('terminalButton').disabled, true);
});

test('a stopped event cannot be overwritten by a late Connect response', async () => {
  const f = await fixture(); const pending = f.run('connect()');
  f.events.get('tunnel-status')({ payload: { state: 'stopped' } });
  f.resolveStart({ socksPort: 1080 }); await pending;
  assert.equal(f.elements.get('connectedTools').classList.contains('hidden'), true);
  assert.equal(f.elements.get('connectButton').disabled, false);
});

test('tool launch completion and language rendering do not reenable tools after Disconnect', async () => {
  const f = await fixture(); const pending = f.run('connect()');
  f.resolveStart({ socksPort: 1080 }); await pending;
  const opening = f.run('openTool("terminal")');
  f.run('renderLanguage()');
  assert.equal(f.elements.get('terminalButton').disabled, true);
  await f.run('disconnect()');
  f.resolveTool(); await opening;
  assert.equal(f.elements.get('terminalButton').disabled, true);
  assert.equal(f.elements.get('vscodeButton').disabled, true);
});

test('RHTLC dashboard-only profiles omit secrets and require a target before launching SSH tools', async () => {
  const f = await fixture();
  f.elements.get('connectionMode').value = 'rhtlc';
  f.elements.get('rhtlcURL').value = 'rhtlc://lab.invalid/lab?token=test-only';
  f.elements.get('password').value = 'not-to-be-saved';
  f.elements.get('profileName').value = 'Lab';
  f.run('saveProfile()');
  const profiles = JSON.parse(f.saved.get('remoteOcp.connectionProfiles.v2'));
  assert.equal(profiles[0].remoteUser, 'student');
  assert.equal(profiles[0].remotePort, '22');
  assert.equal(JSON.stringify(profiles).includes('test-only'), false);
  assert.equal(JSON.stringify(profiles).includes('not-to-be-saved'), false);
  const pending = f.run('connect()'); f.resolveStart({ socksPort: 1081 }); await pending;
  assert.equal(f.elements.get('openSessionButton').disabled, false);
  assert.equal(f.elements.get('terminalButton').disabled, true);
});
