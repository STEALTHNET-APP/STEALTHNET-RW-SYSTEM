import assert from 'node:assert/strict';
import { test } from 'node:test';
import { localInput, withOffset, zoneLabel } from './maintenanceTime.ts';

test('время окончания уходит на сервер с поясом браузера', () => {
  process.env.TZ = 'Europe/Moscow';
  assert.equal(withOffset('2026-11-26T15:00'), '2026-11-26T15:00:00+03:00');
  assert.equal(zoneLabel('2026-11-26T15:00'), 'UTC+3');
  process.env.TZ = 'America/New_York';
  assert.equal(withOffset('2026-11-26T15:00'), '2026-11-26T15:00:00-05:00');
  assert.equal(withOffset('2026-07-01T09:30'), '2026-07-01T09:30:00-04:00');
  assert.equal(zoneLabel('2026-11-26T15:00'), 'UTC−5');
  process.env.TZ = 'Asia/Kolkata';
  assert.equal(withOffset('2026-11-26T15:00'), '2026-11-26T15:00:00+05:30');
  assert.equal(zoneLabel('2026-11-26T15:00'), 'UTC+5:30');
  process.env.TZ = 'UTC';
  assert.equal(zoneLabel('2026-11-26T15:00'), 'UTC');
  assert.equal(withOffset(''), null);
  assert.equal(withOffset('не дата'), null);
});

test('сохранённое время показывается в поле в поясе браузера', () => {
  process.env.TZ = 'Europe/Moscow';
  assert.equal(localInput('2026-11-26T12:00:00Z'), '2026-11-26T15:00');
  process.env.TZ = 'UTC';
  assert.equal(localInput('2026-11-26T12:00:00Z'), '2026-11-26T12:00');
  assert.equal(localInput(null), '');
  assert.equal(localInput('мусор'), '');
});
