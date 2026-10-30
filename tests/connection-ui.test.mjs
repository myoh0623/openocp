import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import vm from 'node:vm';
import test from 'node:test';
import { normalizeLanguage, translate } from '../src/i18n.js';
import { createTutorial } from '../src/tutorial.js';
import { profileIssue } from '../src/profile-validation.js';

// Run the actual UI controller with a small DOM and IPC boundary. No native app
// or lab is needed to exercise event ordering, validation, and profile storage.
async function fixture(initialStorage = []) {
  const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
  const elements = new Map();
  const classes = () => {
    const values = new Set();
    return { add: x => values.add(x), remove: x => values.delete(x), contains: x => values.has(x),
      toggle(x, on) { if (on) values.add(x); else values.delete(x); } };
  };
  for (const [, tag, id] of html.matchAll(/(<[^>]*\bid="([^"]+)"[^>]*>)/g)) {
    elements.set(id, { value: '', type: /type="checkbox"/.test(tag) ? 'checkbox' : 'text',
      checked: false, open: false, textContent: '', classList: classes(), handlers: new Map(), attributes: new Map(),
      addEventListener(type, handler) { this.handlers.set(type, handler); },
      dispatch(type) { return this.handlers.get(type)?.({ target: this }); },
      replaceChildren() {}, add() {}, focus() {},
      setAttribute(key, value) { this.attributes.set(key, value); },
      showModal() { this.open = true; },
      close() { this.open = false; this.dispatch('close'); } });
  }
  const saved = new Map(initialStorage), events = new Map(), calls = [];
  let resolveStart, resolveTool, rejectTool;
  let dependencies = { ssh: { available: false, message: "Install OpenSSH" }, vscode: { available: true, message: "Ready" }, remoteSsh: { available: false, message: "Install Remote - SSH" } };
  const start = new Promise(resolve => { resolveStart = resolve; });
  const tool = new Promise((resolve, reject) => { resolveTool = resolve; rejectTool = reject; });
  const context = vm.createContext({
    document: { getElementById: id => { assert.ok(elements.has(id), id); return elements.get(id); },
      documentElement: {}, querySelector: () => ({ classList: classes() }) },
    localStorage: { getItem: k => saved.get(k) ?? null, setItem: (k, v) => saved.set(k, v), removeItem: k => saved.delete(k) },
    window: { __TAURI_INTERNALS__: true, setTimeout() {}, clearTimeout() {} }, navigator: { userAgent: 'Mac' },
    crypto: { randomUUID }, Option: class {}, URL, Intl, appVersion: '2.2.1', normalizeLanguage, translate, createTutorial, profileIssue,
    applyTranslations() {}, listen: async (name, fn) => events.set(name, fn),
    invoke: async (name, args) => {
      calls.push({ name, args });
      if (name === 'detect_tools') return dependencies;
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
  if (!saved.has('remoteOcp.connectionProfiles.v2')) elements.get('sshIP').value = '192.0.2.10';
  return { elements, saved, events, calls, resolveStart, resolveTool, rejectTool, setDependencies: value => { dependencies = value; }, run: code => vm.runInContext(code, context) };
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

test('RHTLC dashboard-only profiles save the URL, omit passwords, and require a target for SSH tools', async () => {
  const f = await fixture();
  f.elements.get('connectionMode').value = 'rhtlc';
  f.elements.get('remoteHost').value = '';
  f.elements.get('rhtlcURL').value = 'rhtlc://lab.invalid/lab?token=test-only';
  f.elements.get('password').value = 'not-to-be-saved';
  f.elements.get('privateKeyPassphrase').value = 'also-not-to-be-saved';
  f.elements.get('profileName').value = 'Lab';
  f.run('saveProfile()');
  const profiles = JSON.parse(f.saved.get('remoteOcp.connectionProfiles.v2'));
  assert.equal(profiles[0].remoteUser, 'student');
  assert.equal(profiles[0].remotePort, '22');
  assert.equal(profiles[0].rhtlcURL, 'rhtlc://lab.invalid/lab?token=test-only');
  assert.equal(profiles[0].sshIP, '192.0.2.10');
  assert.equal(JSON.stringify(profiles).includes('not-to-be-saved'), false);
  const pending = f.run('connect()'); f.resolveStart({ socksPort: 1081 }); await pending;
  assert.equal(f.elements.get('openSessionButton').disabled, false);
  assert.equal(f.elements.get('terminalButton').disabled, true);
  assert.equal(f.elements.get('rhtlcURL').value, profiles[0].rhtlcURL);
  assert.equal(f.elements.get('password').value, '');
  assert.equal(f.elements.get('privateKeyPassphrase').value, '');
});

test('both connection modes restore the exact IP and full RHTLC URL after restarting', async () => {
  for (const mode of ['ssh', 'rhtlc']) {
    const f = await fixture();
    const address = 'rhtlc://https://lab.invalid/session/path?token=test%2Bonly%3D&other=a%26b';
    for (const [id, value] of Object.entries({ connectionMode: mode, profileName: 'Saved lab',
      sshIP: '192.0.2.42', rhtlcURL: address, password: 'private-password', privateKeyPassphrase: 'private-passphrase' })) {
      f.elements.get(id).value = value;
    }
    f.elements.get('saveProfileButton').dispatch('click');
    const stored = f.saved.get('remoteOcp.connectionProfiles.v2');
    assert.ok(stored);
    assert.equal(stored.includes('private-password'), false);
    assert.equal(stored.includes('private-passphrase'), false);
    const restarted = await fixture(f.saved);
    assert.equal(restarted.elements.get('connectionMode').value, mode);
    assert.equal(restarted.elements.get('sshIP').value, '192.0.2.42');
    assert.equal(restarted.elements.get('rhtlcURL').value, address);
    assert.equal(restarted.elements.get('password').value, '');
    assert.equal(restarted.elements.get('privateKeyPassphrase').value, '');
    assert.equal(restarted.elements.get('connectButton').disabled, false);
  }
});

test('profile switching, legacy profiles, new profiles, and deletion do not retain another lab address', async () => {
  const address = 'rhtlc://lab.invalid/lab?token=saved-lab';
  const profiles = [
    { id: 'lab', name: 'Lab', connectionMode: 'rhtlc', sshIP: '192.0.2.20', rhtlcURL: address, socksPort: '1080' },
    { id: 'legacy', name: 'Legacy', sshIP: '192.0.2.30', sshUser: 'student', sshPort: '22', socksPort: '1080' }
  ];
  const f = await fixture([
    ['remoteOcp.connectionProfiles.v2', JSON.stringify(profiles)], ['remoteOcp.lastProfile.v2', 'lab']
  ]);
  assert.equal(f.elements.get('rhtlcURL').value, address);
  f.elements.get('profileSelect').value = 'legacy';
  f.elements.get('profileSelect').dispatch('change');
  assert.equal(f.elements.get('sshIP').value, '192.0.2.30');
  assert.equal(f.elements.get('rhtlcURL').value, '');
  assert.equal(f.elements.get('connectButton').disabled, false);
  f.elements.get('profileSelect').value = 'lab';
  f.elements.get('profileSelect').dispatch('change');
  assert.equal(f.elements.get('sshIP').value, '192.0.2.20');
  assert.equal(f.elements.get('rhtlcURL').value, address);
  f.elements.get('newProfileButton').dispatch('click');
  assert.equal(f.elements.get('sshIP').value, '');
  assert.equal(f.elements.get('rhtlcURL').value, '');
  f.elements.get('profileSelect').value = 'lab';
  f.elements.get('profileSelect').dispatch('change');
  f.elements.get('deleteProfileButton').dispatch('click');
  f.elements.get('confirmDialog').returnValue = 'confirm';
  f.elements.get('confirmDialog').dispatch('close');
  const remaining = JSON.parse(f.saved.get('remoteOcp.connectionProfiles.v2'));
  assert.deepEqual(remaining.map(p => p.id), ['legacy']);
  assert.equal(f.elements.get('rhtlcURL').value, '');
});

test('connecting updates a named profile and retains both addresses for reconnection', async () => {
  const f = await fixture();
  f.elements.get('connectionMode').value = 'rhtlc';
  f.elements.get('profileName').value = 'Lab';
  f.elements.get('rhtlcURL').value = 'rhtlc://lab.invalid/lab?token=old-test-token';
  f.elements.get('saveProfileButton').dispatch('click');
  const updated = 'rhtlc://lab.invalid/lab?token=new-test-token';
  f.elements.get('rhtlcURL').value = updated;
  f.elements.get('sshIP').value = '192.0.2.50';
  const pending = f.elements.get('connectButton').dispatch('click');
  f.resolveStart({ socksPort: 1081 }); await pending;
  const profiles = JSON.parse(f.saved.get('remoteOcp.connectionProfiles.v2'));
  assert.equal(profiles.length, 1);
  assert.equal(profiles[0].rhtlcURL, updated);
  assert.equal(profiles[0].sshIP, '192.0.2.50');
  await f.elements.get('disconnectButton').dispatch('click');
  f.events.get('tunnel-status')({ payload: { state: 'stopped' } });
  assert.equal(f.elements.get('rhtlcURL').value, updated);
  assert.equal(f.elements.get('connectButton').disabled, false);
  await f.elements.get('connectButton').dispatch('click');
  const requests = f.calls.filter(c => c.name === 'start_connection');
  assert.equal(requests.length, 2);
  assert.equal(requests[1].args.config.rhtlcUrl, updated);
  assert.equal(requests[1].args.config.sshIp, '192.0.2.50');
});

test('dependency checks run before connecting and can be refreshed after installation', async () => {
  const f = await fixture();
  assert.equal(f.calls.some(c => c.name === 'detect_tools'), true);
  assert.equal(f.calls.some(c => c.name === 'start_connection'), false);
  assert.equal(f.elements.get('sshReadiness').textContent, 'Install OpenSSH');
  const ready = { available: true, message: 'Ready' };
  f.setDependencies({ ssh: ready, vscode: ready, remoteSsh: ready });
  await f.run('refreshToolsDetection()');
  assert.equal(f.elements.get('sshReadiness').textContent, 'Ready');
  assert.equal(f.elements.get('remoteSshReadiness').className, 'ready');
});

test('launch failures remain visible across language changes and are recorded', async () => {
  const f = await fixture();
  const pending = f.run('connect()'); f.resolveStart({ socksPort: 1080 }); await pending;
  const opening = f.run('openTool("vscode")');
  f.rejectTool('VS Code exited with code 7'); await opening;
  assert.equal(f.elements.get('errorNotice').classList.contains('hidden'), false);
  f.run('renderLanguage()');
  assert.equal(f.elements.get('errorMessage').textContent, 'VS Code exited with code 7');
  assert.match(f.elements.get('logOutput').textContent, /VS Code exited with code 7/);
  f.events.get('tool-error')({ payload: 'A later startup failure' });
  assert.equal(f.elements.get('errorMessage').textContent, 'A later startup failure');
});

test('first-run tutorial is dismissible, repeatable, and does not overwrite the form', async () => {
  const f = await fixture();
  assert.equal(f.elements.get('tutorialDialog').open, true);
  assert.equal(f.elements.get('tutorial-connectionMode').value, 'rhtlc');
  assert.equal(f.elements.get('remoteFolder').value, '/home/student');
  assert.equal(f.elements.get('remoteHost').value, 'workstation.lab.example.com');
  assert.equal(f.elements.get('tutorial-remoteHost').value, 'workstation.lab.example.com');
  f.elements.get('tutorial-rhtlcURL').value = 'rhtlc://lab.invalid/session?token=discard-me';
  f.elements.get('tutorialLaterButton').dispatch('click');
  assert.equal(f.elements.get('tutorialDialog').open, false);
  assert.equal(f.elements.get('rhtlcURL').value, '');
  assert.equal(f.elements.get('tutorial-rhtlcURL').value, '');
  assert.equal(f.saved.has('remoteOcp.connectionProfiles.v2'), false);
  const restarted = await fixture(f.saved);
  assert.equal(restarted.elements.get('tutorialDialog').open, false);
  restarted.elements.get('tutorialButton').dispatch('click');
  assert.equal(restarted.elements.get('tutorialDialog').open, true);
});

test('skipping every address saves a draft without connecting, and survives restart', async () => {
  for (const mode of ['rhtlc', 'ssh']) {
    const f = await fixture();
    f.elements.get('tutorial-connectionMode').value = mode;
    f.elements.get('tutorial-connectionMode').dispatch('change');
    for (let i = 0; i < 3; i++) f.elements.get('tutorialSkipButton').dispatch('click');
    assert.equal(f.elements.get('tutorialStep3').hidden, false);
    assert.match(f.elements.get('tutorialCompletionHint').textContent, /missing connection address/);
    f.elements.get('tutorialNextButton').dispatch('click');
    const profile = JSON.parse(f.saved.get('remoteOcp.connectionProfiles.v2'))[0];
    assert.equal(profile.connectionMode, mode);
    assert.equal(profile.sshIP, '');
    assert.equal(profile.rhtlcURL, '');
    assert.equal(profile.remoteFolder, '/home/student');
    assert.equal(profile.remoteHost, '');
    assert.equal(f.elements.get('connectButton').disabled, true);
    await f.run('connect()');
    assert.equal(f.calls.some(c => c.name === 'start_connection'), false);
    const restarted = await fixture(f.saved);
    assert.equal(restarted.elements.get('tutorialDialog').open, false);
    assert.equal(restarted.elements.get('connectButton').disabled, true);
  }
});

test('tutorial preserves entered values across Back and language changes, masks tokens, and saves once', async () => {
  const f = await fixture();
  const address = 'rhtlc://https://lab.invalid/session?token=test%2Bonly%3D&other=a%26b';
  const entered = { profileName: 'My lab', rhtlcURL: address,
    remoteHost: 'workstation.lab.example.com', remoteUser: 'student', remotePort: '22',
    remoteFolder: '/home/student/my project', dashboardURL: 'https://console.apps.lab.invalid/' };
  for (const [field, value] of Object.entries(entered)) f.elements.get(`tutorial-${field}`).value = value;
  f.elements.get('password').value = 'never-store-this';
  f.elements.get('privateKeyPassphrase').value = 'never-store-this-either';
  f.elements.get('tutorialNextButton').dispatch('click');
  f.elements.get('tutorialBackButton').dispatch('click');
  f.elements.get('tutorialLanguage').value = 'ko';
  await f.elements.get('tutorialLanguage').dispatch('change');
  assert.equal(f.elements.get('tutorial-rhtlcURL').value, address);
  assert.equal(f.elements.get('tutorialStepTitle').textContent, '연결 주소');
  for (let i = 0; i < 3; i++) f.elements.get('tutorialNextButton').dispatch('click');
  assert.equal(f.elements.get('tutorialStep3').hidden, false);
  assert.equal(f.elements.get('tutorialSummaryConnection').textContent.includes('test%2B'), false);
  f.elements.get('tutorialNextButton').dispatch('click');
  const profiles = JSON.parse(f.saved.get('remoteOcp.connectionProfiles.v2'));
  assert.equal(profiles.length, 1);
  assert.equal(profiles[0].name, entered.profileName);
  for (const [field, value] of Object.entries(entered).filter(([field]) => field !== 'profileName')) assert.equal(profiles[0][field], value);
  assert.equal(JSON.stringify(profiles).includes('never-store'), false);
  assert.equal(f.calls.some(c => c.name === 'start_connection'), false);
  assert.equal(f.elements.get('connectButton').disabled, false);
  assert.equal(f.elements.get('tutorial-rhtlcURL').value, '');
});

test('tutorial rejects malformed supplied values before advancing', async () => {
  const f = await fixture();
  f.elements.get('tutorial-rhtlcURL').value = 'https://rol.redhat.com/rol/app/';
  f.elements.get('tutorialNextButton').dispatch('click');
  assert.equal(f.elements.get('tutorialStep0').hidden, false);
  assert.equal(f.elements.get('tutorialError').textContent, translate('en', 'invalidRhtlc'));
  f.elements.get('tutorial-rhtlcURL').value = '';
  f.elements.get('tutorial-connectionMode').value = 'ssh';
  f.elements.get('tutorial-sshIP').value = '999.1.2.3';
  f.elements.get('tutorialNextButton').dispatch('click');
  assert.equal(f.elements.get('tutorialError').textContent, translate('en', 'invalidIP'));
  f.elements.get('tutorial-connectionMode').value = 'rhtlc';
  f.elements.get('tutorialNextButton').dispatch('click');
  for (const [field, value, key] of [
    ['remoteHost', 'https://workstation.lab.example.com', 'invalidRemoteHost'],
    ['remoteFolder', 'relative/path', 'invalidRemoteFolder']
  ]) {
    f.elements.get(`tutorial-${field}`).value = value;
    f.elements.get('tutorialNextButton').dispatch('click');
    assert.equal(f.elements.get('tutorialStep1').hidden, false);
    assert.equal(f.elements.get('tutorialError').textContent, translate('en', key));
    f.elements.get(`tutorial-${field}`).value = '';
  }
  f.elements.get('tutorialNextButton').dispatch('click');
  f.elements.get('tutorial-dashboardURL').value = 'file:///private/file';
  f.elements.get('tutorialNextButton').dispatch('click');
  assert.equal(f.elements.get('tutorialStep2').hidden, false);
  assert.equal(f.saved.has('remoteOcp.connectionProfiles.v2'), false);
});

test('existing profiles keep explicit folders and advanced settings when using or cancelling the tutorial', async () => {
  for (const folder of [undefined, '', '/opt/project']) {
    const profile = { id: 'saved', name: 'Saved', connectionMode: 'rhtlc', rhtlcURL: 'rhtlc://lab.invalid/?token=original',
      socksPort: '1081', browserPath: '/custom/browser', privateKeyPath: '/custom/key', remoteFolder: folder };
    const storage = [['remoteOcp.connectionProfiles.v2', JSON.stringify([profile])]];
    const f = await fixture(storage);
    assert.equal(f.elements.get('tutorialDialog').open, false);
    assert.equal(f.elements.get('remoteFolder').value, folder ?? '/home/student');
    f.elements.get('tutorialButton').dispatch('click');
    f.elements.get('tutorial-remoteFolder').value = '/discard';
    f.elements.get('tutorialDialog').close(); // Native dialog Escape has the same close event.
    assert.equal(f.elements.get('remoteFolder').value, folder ?? '/home/student');
    assert.equal(f.saved.get('remoteOcp.connectionProfiles.v2'), storage[0][1]);
    f.elements.get('tutorialButton').dispatch('click');
    for (let i = 0; i < 4; i++) f.elements.get('tutorialNextButton').dispatch('click');
    const saved = JSON.parse(f.saved.get('remoteOcp.connectionProfiles.v2'));
    assert.equal(saved.length, 1);
    assert.equal(saved[0].id, 'saved');
    assert.equal(saved[0].remoteFolder, folder ?? '/home/student');
    assert.equal(saved[0].browserPath, '/custom/browser');
    assert.equal(saved[0].privateKeyPath, '/custom/key');
    assert.equal(saved[0].socksPort, '1081');
  }
});

test('tutorial cannot modify an active connection', async () => {
  const f = await fixture();
  f.elements.get('tutorialLaterButton').dispatch('click');
  const pending = f.run('connect()');
  f.elements.get('tutorialButton').dispatch('click');
  assert.equal(f.elements.get('tutorialDialog').open, false);
  assert.equal(f.elements.get('tutorialButton').disabled, true);
  f.resolveStart({ socksPort: 1080 }); await pending;
  f.elements.get('tutorialButton').dispatch('click');
  assert.equal(f.elements.get('tutorialDialog').open, false);
});
