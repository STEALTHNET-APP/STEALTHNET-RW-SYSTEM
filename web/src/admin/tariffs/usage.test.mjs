import assert from 'node:assert/strict';
import test from 'node:test';
import { heldText, holderName } from './usage.ts';

test('сколько подписок на тарифе, с правильными окончаниями', () => {
  assert.equal(heldText({ active: 1, ended: 0 }), '1 действующая подписка');
  assert.equal(heldText({ active: 2, ended: 0 }), '2 действующие подписки');
  assert.equal(heldText({ active: 5, ended: 0 }), '5 действующих подписок');
  assert.equal(heldText({ active: 11, ended: 0 }), '11 действующих подписок');
  assert.equal(heldText({ active: 21, ended: 0 }), '21 действующая подписка');
  assert.equal(heldText({ active: 0, ended: 3 }), '3 закончившиеся подписки');
  assert.equal(heldText({ active: 2, ended: 1 }), '3 подписки, из них закончилась 1');
  assert.equal(heldText({ active: 1, ended: 4 }), '5 подписок, из них закончились 4');
});

test('клиент называется именем, ником, почтой или номером', () => {
  const base = { client_id: 7, name: null, telegram_username: null, email: null };
  assert.equal(holderName({ ...base, name: 'Алексей' }), 'Алексей');
  assert.equal(holderName({ ...base, name: '  ', telegram_username: 'alex' }), '@alex');
  assert.equal(holderName({ ...base, email: 'a@example.com' }), 'a@example.com');
  assert.equal(holderName(base), 'Клиент 7');
});
