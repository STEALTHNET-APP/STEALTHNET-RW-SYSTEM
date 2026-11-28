import assert from 'node:assert/strict';
import { test } from 'node:test';
import { removedUntil } from './removeDays.ts';

const NOW = Date.parse('2026-11-28T12:00:00Z');

test('срок сдвигается на убранные дни', () => {
  assert.deepEqual(removedUntil('2026-12-28T12:00:00Z', 7, NOW), { until: '2026-12-21T12:00:00.000Z', ends: false });
});

test('если дней осталось меньше, подписка заканчивается сразу', () => {
  assert.deepEqual(removedUntil('2026-12-01T12:00:00Z', 30, NOW), { until: '2026-11-28T12:00:00.000Z', ends: true });
});

test('у закончившейся подписки и без дней убирать нечего', () => {
  assert.equal(removedUntil('2026-11-20T12:00:00Z', 7, NOW), null);
  assert.equal(removedUntil(null, 7, NOW), null);
  assert.equal(removedUntil('2026-12-28T12:00:00Z', 0, NOW), null);
});
