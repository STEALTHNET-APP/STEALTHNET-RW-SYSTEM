import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EMPTY, activeKinds, apiPairs, clearKind, conditionsJson, fromAudience, label, readParams, toAudience, writeParams } from './filters.ts';

const options = {
  tariffs: [
    { id: 1, name: 'Стандарт', category_id: 1, deleted: false },
    { id: 3, name: 'Семейный', category_id: 1, deleted: false },
    { id: 2, name: 'Unblock', category_id: 2, deleted: false },
  ],
  categories: [
    { id: 1, name: 'Стандарт' },
    { id: 2, name: 'Unblock' },
  ],
  utm: ['tiktok'],
};

test('адрес страницы читается и пишется без потерь', () => {
  const params = new URLSearchParams('tariff=3,1,3&status=active&source=bot,web,bad&paid=yes&paid_from=500&from=2026-11-01&to=2026-11-25&utm=tiktok&q=иван');
  const cond = readParams(params);
  assert.deepEqual(cond.tariff, [3, 1]);
  assert.equal(cond.status, 'active');
  assert.deepEqual(cond.source, ['bot', 'web']);
  assert.equal(cond.paid, true);
  assert.equal(cond.paid_from, 500);
  assert.equal(cond.from, '2026-11-01');
  const written = writeParams(cond, params);
  assert.equal(written.get('q'), 'иван');
  assert.equal(written.get('tariff'), '3,1');
  assert.deepEqual(readParams(written), cond);
});

test('старый адрес с segment и мусор в адресе', () => {
  assert.equal(readParams(new URLSearchParams('segment=expiring_soon')).status, 'expiring_soon');
  const junk = readParams(new URLSearchParams('status=lost&ends=0&ended=400&lang=de&tariff=x,-1&from=2026-13'));
  assert.deepEqual(junk, EMPTY);
});

test('даты уходят в API началом дня и днём после конца', () => {
  const cond = { ...EMPTY, from: '2026-11-01', to: '2026-11-25' };
  const pairs = Object.fromEntries(apiPairs(cond));
  assert.equal(pairs.from, new Date(2026, 10, 1).toISOString());
  assert.equal(pairs.to, new Date(2026, 10, 26).toISOString());
  const back = fromAudience(toAudience(cond));
  assert.equal(back.from, '2026-11-01');
  assert.equal(back.to, '2026-11-25');
});

test('аудитория рассылки и условия правила', () => {
  const cond = { ...EMPTY, tariff: [1], autorenew: false, invited: true };
  assert.deepEqual(toAudience(cond), { kind: 'filter', tariff: [1], autorenew: false, invited: true });
  assert.deepEqual(conditionsJson(cond), { tariff: [1], autorenew: false, invited: true });
  assert.equal(conditionsJson(EMPTY), null);
  assert.deepEqual(fromAudience({ kind: 'filter', tariff: [1], autorenew: false, invited: true }), cond);
});

test('подписи плашек', () => {
  const cond = { ...EMPTY, tariff: [1, 3, 2], source: ['bot'], ends: 7, paid_from: 1500, telegram: true, email: false };
  assert.equal(label('tariff', cond, options, 'RUB'), 'Тарифы: Стандарт, Семейный и ещё 1');
  assert.equal(label('source', cond, options, 'RUB'), 'Откуда: бот');
  assert.equal(label('term', cond, options, 'RUB'), 'Закончится в ближайшие 7 дней');
  assert.equal(label('paid', cond, options, 'RUB'), 'Оплатили от 1 500 ₽');
  assert.equal(label('paid', cond, options, 'USD'), 'Оплатили от $1 500');
  assert.equal(label('contact', cond, options, 'RUB'), 'Есть Telegram, без почты');
  assert.deepEqual(activeKinds(cond, true), ['tariff', 'term', 'source', 'contact', 'paid']);
  assert.deepEqual(activeKinds(clearKind(cond, 'contact'), true), ['tariff', 'term', 'source', 'paid']);
});
