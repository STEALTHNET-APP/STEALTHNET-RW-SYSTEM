import { test } from 'node:test';
import assert from 'node:assert/strict';
import { categoryOffers, categoryPick, firstPick, flatOffers, offerCategories, renewsCurrent, tariffPick, targetPick } from './offers.ts';

const period = (id, months) => ({ period_id: id, months, days: months * 30 });
const offer = (id, category, included, max, periods) => ({
  category: { id: category, name: `Категория ${category}`, sort: category },
  tariff: { id, name: `Тариф ${id}`, devices: { included, extra: max === null ? null : { max } } },
  periods,
});

const first = offer(1, 10, 3, 5, [period(11, 1), period(12, 3), period(13, 12)]);
const second = offer(2, 10, 5, 5, [period(21, 1), period(22, 3), period(23, 6)]);
const other = offer(3, 20, 2, null, [period(31, 1), period(33, 12)]);
const offers = [first, second, other];
const ownSecond = { tariff_id: 2, period_days: 180, extra_devices: 2 };

test('оба тарифа одной категории можно выбрать', () => {
  assert.deepEqual(offerCategories(offers).map((c) => c.id), [10, 20]);
  assert.deepEqual(categoryOffers(offers, 10).map((o) => o.tariff.id), [1, 2]);
  const start = firstPick(offers, null);
  assert.deepEqual(start, { tariffId: 1, periodId: 12, devices: 3 });
  assert.deepEqual(tariffPick(offers, 2, null, start), { tariffId: 2, periodId: 22, devices: 5 });
  assert.deepEqual(tariffPick(offers, 1, null, tariffPick(offers, 2, null, start)), { tariffId: 1, periodId: 12, devices: 3 });
});

test('продление второго тарифа категории оставляет второй', () => {
  assert.deepEqual(firstPick(offers, ownSecond), { tariffId: 2, periodId: 23, devices: 7 });
  assert.deepEqual(targetPick(offers, ownSecond, { tariffId: 1, periodId: 12, devices: 3 }), { tariffId: 2, periodId: 23, devices: 7 });
  assert.deepEqual(categoryPick(offers, 10, ownSecond, { tariffId: 3, periodId: 31, devices: 2 }), { tariffId: 2, periodId: 21, devices: 7 });
});

test('при возврате в категорию выбирается свой тариф', () => {
  const start = firstPick(offers, ownSecond);
  const away = categoryPick(offers, 20, ownSecond, start);
  assert.deepEqual(away, { tariffId: 3, periodId: 33, devices: 2 });
  assert.deepEqual(categoryPick(offers, 10, ownSecond, away), { tariffId: 2, periodId: 23, devices: 7 });
  assert.deepEqual(categoryPick(offers, 10, null, away), { tariffId: 1, periodId: 13, devices: 3 });
});

test('нажатие на выбранную категорию не сбрасывает выбранный тариф', () => {
  const chosen = { tariffId: 2, periodId: 21, devices: 6 };
  assert.equal(categoryPick(offers, 10, null, chosen), chosen);
  assert.equal(tariffPick(offers, 2, null, chosen), chosen);
});

test('докупленные устройства остаются только у своего тарифа', () => {
  const own = firstPick(offers, ownSecond);
  assert.equal(tariffPick(offers, 1, ownSecond, own).devices, 3);
  assert.equal(tariffPick(offers, 2, ownSecond, tariffPick(offers, 1, ownSecond, own)).devices, 7);
  assert.equal(firstPick(offers, { ...ownSecond, extra_devices: 9 }).devices, 10);
  assert.equal(firstPick([offer(4, 10, 2, null, [period(41, 1)])], { tariff_id: 4, period_days: 30, extra_devices: 3 }).devices, 2);
});

test('срок сохраняется, если у нового тарифа такой есть', () => {
  assert.equal(tariffPick(offers, 2, null, { tariffId: 1, periodId: 11, devices: 3 }).periodId, 21);
  assert.equal(tariffPick(offers, 2, null, { tariffId: 1, periodId: 12, devices: 3 }).periodId, 22);
  assert.equal(tariffPick(offers, 2, null, { tariffId: 1, periodId: 13, devices: 3 }).periodId, 22);
  assert.equal(tariffPick(offers, 2, ownSecond, { tariffId: 1, periodId: 13, devices: 3 }).periodId, 23);
  assert.equal(categoryPick(offers, 20, null, { tariffId: 1, periodId: 13, devices: 3 }).periodId, 33);
});

test('ссылка на тариф и срок открывает именно их', () => {
  assert.deepEqual(firstPick(offers, ownSecond, { tariffId: 1, periodId: 13 }), { tariffId: 1, periodId: 13, devices: 3 });
  assert.deepEqual(firstPick(offers, null, { tariffId: 2, periodId: 99 }), { tariffId: 2, periodId: 22, devices: 5 });
  assert.deepEqual(firstPick(offers, ownSecond, { tariffId: Number.NaN }), { tariffId: 2, periodId: 23, devices: 7 });
});

test('без своего тарифа в каталоге выбор остаётся прежним', () => {
  const chosen = { tariffId: 2, periodId: 22, devices: 7 };
  assert.deepEqual(targetPick(offers, null, chosen), { tariffId: 2, periodId: 22, devices: 5 });
  assert.deepEqual(targetPick(offers, { tariff_id: 99, period_days: 30, extra_devices: 1 }, chosen), { tariffId: 2, periodId: 22, devices: 5 });
  assert.deepEqual(targetPick(offers, null, null), { tariffId: 1, periodId: 12, devices: 3 });
  assert.equal(firstPick([], ownSecond), null);
});

test('смену тарифа внутри категории сервер считает продлением', () => {
  assert.equal(renewsCurrent(first, second), true);
  assert.equal(renewsCurrent(second, second), true);
  assert.equal(renewsCurrent(second, other), false);
  assert.equal(renewsCurrent(null, second), null);
});

test('тарифы с одним сроком показываются сразу все, если условия одинаковые', () => {
  const single = (id, category, sort, days) => ({
    category: { id: category, name: `Категория ${category}`, sort },
    tariff: { id, name: `Тариф ${id}`, devices: { included: 7, extra: null } },
    periods: [{ period_id: id * 10, months: Math.max(1, Math.round(days / 30)), days }],
  });
  const week = single(1, 10, 1, 7);
  const month = single(2, 20, 2, 30);
  const year = single(3, 30, 3, 365);
  const terms = () => 'семь устройств';
  assert.deepEqual(flatOffers([year, week, month], terms).map((o) => o.tariff.id), [1, 2, 3]);
  assert.equal(flatOffers([week], terms), null);
  assert.equal(flatOffers([week, first], terms), null);
  assert.equal(flatOffers([week, single(4, 40, 4, 7)], terms), null);
  assert.equal(flatOffers([week, month], (o) => (o.tariff.id === 1 ? 'одно устройство' : 'семь устройств')), null);
});
