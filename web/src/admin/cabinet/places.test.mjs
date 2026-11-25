import { registerHooks } from 'node:module';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

registerHooks({
  resolve(specifier, context, next) {
    if (specifier.startsWith('.') && !/\.[a-z]+$/.test(specifier)) {
      return next(`${specifier}.ts`, context);
    }
    return next(specifier, context);
  },
});

const { PARTS, partOf } = await import('../../cabinet/parts.ts');
const { CABINET_PLACES, PLACE_ABOUT, PLACE_NAMES, placeHint, placeOf, placeSample, placeTitle, SCREEN_NAMES, screenOf, SNIPPETS } = await import('./places.ts');

const markup = readdirSync(new URL('../../cabinet/', import.meta.url))
  .filter((name) => name.endsWith('.tsx'))
  .map((name) => readFileSync(new URL(`../../cabinet/${name}`, import.meta.url), 'utf8'))
  .join('\n');

test('у каждого места есть разметка в кабинете', () => {
  assert.deepEqual(CABINET_PLACES, ['page', ...PARTS]);
  const routed = new Set(['/cabinet', '/cabinet/pay', '/cabinet/addons', '/cabinet/topup', '/cabinet/payments', '/cabinet/devices', '/cabinet/friends', '/cabinet/gifts', '/cabinet/help/2', '/cabinet/security'].map(partOf));
  for (const part of PARTS) {
    assert.ok(routed.has(part) || markup.includes(`data-part="${part}"`) || markup.includes(`part="${part}"`), part);
  }
  assert.equal(partOf('/cabinet/payments'), 'money');
  assert.equal(partOf('/cabinet/paying'), 'subscription');
});

test('у каждого места есть название, подсказка, пример и заготовки', () => {
  assert.equal(new Set(CABINET_PLACES.map((place) => PLACE_NAMES[place])).size, CABINET_PLACES.length);
  for (const place of CABINET_PLACES) {
    assert.ok(PLACE_ABOUT[place], place);
    assert.ok(placeHint(place).startsWith('Действует'), place);
    assert.ok(placeTitle(place).length > 0, place);
    assert.match(placeSample(place), /^[.a-z][\w-]* \{\n {2}[a-z-]+: [^;]+;\n\}$/, place);
    assert.ok(SNIPPETS[place].length >= 2, place);
    assert.equal(new Set(SNIPPETS[place].map((item) => item.name)).size, SNIPPETS[place].length, place);
  }
  assert.deepEqual(Object.keys(SNIPPETS).sort(), [...CABINET_PLACES].sort());
});

test('место и экран превью сходятся', () => {
  for (const { key } of SCREEN_NAMES) {
    const place = placeOf(key);
    assert.ok(CABINET_PLACES.includes(place), key);
    assert.equal(screenOf(place, key), key, key);
  }
  assert.equal(screenOf('connect', 'pay'), 'subscription');
  assert.equal(screenOf('header', 'login'), 'subscription');
  assert.equal(screenOf('page', 'login'), 'login');
});
