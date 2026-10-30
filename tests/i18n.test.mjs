import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import messages from '../src/locales.json' with { type: 'json' };
import { normalizeLanguage, translate } from '../src/i18n.js';

const placeholders = (value) => [...value.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

test('unsupported or absent preferences fall back to English', () => {
  for (const value of [null, undefined, '', 'fr', 'en-US']) assert.equal(normalizeLanguage(value), 'en');
  assert.equal(normalizeLanguage('ko'), 'ko');
  assert.equal(translate('fr', 'connect'), 'Connect');
});

test('both languages cover every screen label and use matching placeholders', () => {
  for (const [key, entry] of Object.entries(messages)) {
    assert.ok(entry.en?.trim(), `${key}: English is missing`);
    assert.ok(entry.ko?.trim(), `${key}: Korean is missing`);
    assert.deepEqual(placeholders(entry.en), placeholders(entry.ko), key);
  }
  const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
  const main = ['main.js', 'tutorial.js'].map(file => readFileSync(new URL(`../src/${file}`, import.meta.url), 'utf8')).join('\n');
  const keys = [...html.matchAll(/data-i18n(?:-[a-z-]+)?="([^"]+)"/g), ...main.matchAll(/\bt\("([^"]+)"/g)];
  for (const [, key] of keys) assert.ok(messages[key], `Missing UI translation: ${key}`);
});

test('profile names, URLs, and ports are interpolated without modifying user text', () => {
  const name = 'Training {port} <admin>';
  assert.equal(translate('en', 'profileSaved', { name }), `Saved profile “${name}”.`);
  assert.equal(translate('ko', 'profileSaved', { name }), `‘${name}’ 프로필을 저장했습니다.`);
  assert.equal(translate('en', 'connected', { port: 1081 }), 'Connected · 127.0.0.1:1081');
});
