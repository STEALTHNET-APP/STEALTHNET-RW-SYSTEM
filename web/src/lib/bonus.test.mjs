import assert from 'node:assert/strict';
import test from 'node:test';
import { bonusOf, bonusReached, bonusTaken } from './bonus.ts';

test('бонус округляется вниз до копейки', () => {
  const at = (minor, percent) => bonusOf(minor, { percent });
  assert.equal(at(50_000, 5), 2_500);
  assert.equal(at(1_005, 7), 70);
  assert.equal(at(99, 1), 0);
  assert.equal(at(10_700, 5), 535);
  assert.equal(at(-100, 5), 0);
  assert.equal(bonusOf(50_000, null), 0);
  assert.equal(bonusOf(50_000, undefined), 0);
});

test('порог и предел бонуса', () => {
  const terms = { percent: 10, min: { minor: 50_000 }, max: { minor: 8_000 } };
  assert.equal(bonusOf(49_999, terms), 0);
  assert.equal(bonusReached(49_999, terms), false);
  assert.equal(bonusOf(50_000, terms), 5_000);
  assert.equal(bonusReached(50_000, terms), true);
  assert.equal(bonusOf(80_000, terms), 8_000);
  assert.equal(bonusOf(1_000_000, terms), 8_000);
});

test('возврат по частям снимает ровно начисленный бонус', () => {
  let remaining = 1_005;
  let left = 70;
  const taken = [];
  for (const refund of [333, 333, 339]) {
    const part = bonusTaken(left, remaining, refund);
    taken.push(part);
    left -= part;
    remaining -= refund;
  }
  assert.deepEqual(taken, [24, 23, 23]);
  assert.equal(left, 0);
});

test('без бонуса и без суммы ничего не снимается', () => {
  assert.equal(bonusTaken(0, 1_000, 500), 0);
  assert.equal(bonusTaken(50, 1_000, 0), 0);
  assert.equal(bonusTaken(50, 1_000, 5_000), 50);
});
