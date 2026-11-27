import { test } from 'node:test';
import assert from 'node:assert/strict';
import { changePercent, dayLabel, lastWeek, todayChange, totals } from './weekStats.ts';

const rub = (r) => ({ minor: r * 100, currency: 'RUB' });

function days(count, money = true) {
  return Array.from({ length: count }, (_, i) => {
    const day = new Date(Date.UTC(2026, 10, 27 - (count - 1 - i))).toISOString().slice(0, 10);
    return {
      day,
      new_clients: 10 + i,
      new_clients_paid: money ? i : null,
      revenue: money ? rub(1000 * (i + 1)) : null,
      payments: money ? 2 * (i + 1) : null,
    };
  });
}

test('неделя — последние 7 дней ряда, сегодня последним', () => {
  const week = lastWeek(days(14));
  assert.deepEqual(
    week.rows.map((r) => [r.day, r.back]),
    [
      ['2026-11-21', 6],
      ['2026-11-22', 5],
      ['2026-11-23', 4],
      ['2026-11-24', 3],
      ['2026-11-25', 2],
      ['2026-11-26', 1],
      ['2026-11-27', 0],
    ],
  );
  assert.deepEqual(week.totals, {
    revenue: rub(8000 + 9000 + 10000 + 11000 + 12000 + 13000 + 14000),
    payments: 2 * (8 + 9 + 10 + 11 + 12 + 13 + 14),
    newClients: 17 + 18 + 19 + 20 + 21 + 22 + 23,
    newPaid: 7 + 8 + 9 + 10 + 11 + 12 + 13,
  });
  assert.equal(lastWeek(days(3)).rows.length, 3);
  assert.equal(lastWeek([]), null);
});

test('без права на деньги суммы и оплаты не складываются, новые клиенты считаются', () => {
  assert.deepEqual(totals(days(7, false)), { revenue: null, payments: null, newClients: 10 + 11 + 12 + 13 + 14 + 15 + 16, newPaid: null });
});

test('изменение к прошлым 7 дням и подписи дней', () => {
  assert.equal(changePercent(rub(1_043_201), rub(1_100_000)), -5);
  assert.equal(changePercent(rub(120), rub(100)), 20);
  assert.equal(changePercent(rub(120), rub(0)), null);
  assert.equal(changePercent(rub(120), undefined), null);
  assert.equal(changePercent(null, rub(100)), null);
  assert.equal(dayLabel('2026-11-27', 0), 'сегодня');
  assert.equal(dayLabel('2026-11-26', 1), 'вчера');
  assert.equal(dayLabel('2026-11-21', 6), 'сб 21');
  assert.equal(dayLabel('2026-11-23', 4), 'пн 23');
  assert.equal(dayLabel('2026-11-01', 26), 'вс 1');
});

test('сегодня сравнивается со вчера к этому же времени, без него — с полным вчерашним днём', () => {
  assert.deepEqual(todayChange({ today: rub(1200), yesterday: rub(3000), yesterday_same_time: rub(1000) }), { percent: 20, versus: 'к этому времени вчера' });
  assert.deepEqual(todayChange({ today: rub(1200), yesterday: rub(3000) }), { percent: -60, versus: 'к вчерашнему дню' });
  assert.equal(todayChange({ today: rub(1200), yesterday: rub(3000), yesterday_same_time: rub(0) }), null);
  assert.equal(todayChange(null), null);
});
