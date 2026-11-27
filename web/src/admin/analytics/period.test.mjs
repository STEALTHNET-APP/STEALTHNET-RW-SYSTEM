import { test } from 'node:test';
import assert from 'node:assert/strict';
import { periodText, presetDates, presetFromParam, PRESETS, rollingPreset } from './period.ts';

test('«Сегодня» и «Вчера» — один день от сегодняшней даты панели', () => {
  assert.deepEqual(presetDates('today', '2026-11-27'), { from: '2026-11-27', to: '2026-11-27' });
  assert.deepEqual(presetDates('yesterday', '2026-11-27'), { from: '2026-11-26', to: '2026-11-26' });
  assert.deepEqual(presetDates('yesterday', '2026-03-01'), { from: '2026-02-28', to: '2026-02-28' });
  assert.deepEqual(presetDates('yesterday', '2026-01-01'), { from: '2025-12-31', to: '2025-12-31' });
  assert.deepEqual(presetDates('7', '2026-11-27'), { from: '2026-11-21', to: '2026-11-27' });
  assert.deepEqual(PRESETS.slice(0, 2), [
    ['today', 'Сегодня'],
    ['yesterday', 'Вчера'],
  ]);
});

test('скользящие 7, 30 и 90 дней сравниваются с предыдущими днями, а не с теми же числами месяца', () => {
  assert.deepEqual(
    PRESETS.map(([p]) => p).filter(rollingPreset),
    ['7', '30', '90'],
  );
});

test('период из адреса принимается только из списка', () => {
  assert.equal(presetFromParam('yesterday'), 'yesterday');
  assert.equal(presetFromParam('7'), '7');
  assert.equal(presetFromParam('custom'), null);
  assert.equal(presetFromParam('week'), null);
  assert.equal(presetFromParam(null), null);
});

test('один день подписывается датой, даже первого числа', () => {
  assert.equal(periodText('2026-04-01', '2026-04-01'), '1 апреля 2026');
  assert.equal(periodText('2026-11-26', '2026-11-26'), '26 ноября 2026');
  assert.equal(periodText('2026-04-01', '2026-04-30'), 'Апрель 2026');
  assert.equal(periodText('2026-04-01', '2026-04-15'), 'Апрель 2026, по 15');
});
