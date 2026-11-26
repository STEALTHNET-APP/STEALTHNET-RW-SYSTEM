import { test } from 'node:test';
import assert from 'node:assert/strict';
import { appendPage, collect, cursorOf, listPath, readDays, readFilter } from './list.ts';

const item = (id) => ({ ticket: { id } });

function server(total, keyOf = (id) => id) {
  const all = Array.from({ length: total }, (_, i) => item(i + 1));
  const calls = [];
  const fetchPage = async (after, limit) => {
    calls.push({ after, limit });
    const start = after ? all.findIndex((x) => x.ticket.id === after.id) + 1 : 0;
    const items = all.slice(start, start + limit);
    const more = start + limit < all.length;
    const last = items[items.length - 1];
    return { items, next: more ? keyOf(last.ticket.id) : null, next_id: more ? last.ticket.id : null, counts: { waiting: total } };
  };
  return { fetchPage, calls };
}

test('курсор строится и при ключе 0, у которого нет пары — нет курсора', () => {
  assert.deepEqual(cursorOf({ next: 0, next_id: 41 }), { key: 0, id: 41 });
  assert.deepEqual(cursorOf({ next: 1250, next_id: 7 }), { key: 1250, id: 7 });
  assert.equal(cursorOf({ next: null, next_id: null }), null);
  assert.equal(cursorOf({ next: 12 }), null);
});

test('адрес части списка несёт фильтр, поиск и курсор', () => {
  assert.equal(listPath('waiting', '', null, 50), '/api/admin/support/tickets?filter=waiting&limit=50');
  assert.equal(
    listPath('closed', 'не подключается', { key: 0, id: 41 }, 50),
    '/api/admin/support/tickets?filter=closed&limit=50&q=%D0%BD%D0%B5+%D0%BF%D0%BE%D0%B4%D0%BA%D0%BB%D1%8E%D1%87%D0%B0%D0%B5%D1%82%D1%81%D1%8F&after=0&after_id=41',
  );
  assert.match(listPath('answered', '', null, 900), /limit=200$/);
  assert.match(listPath('answered', '', null, 0), /limit=1$/);
});

test('новая часть добавляется без повторов', () => {
  assert.deepEqual(
    appendPage([item(1), item(2)], [item(2), item(3)]).map((x) => x.ticket.id),
    [1, 2, 3],
  );
});

test('обновление перечитывает столько, сколько уже показано, частями не больше 200', async () => {
  const one = server(130, () => 0);
  const first = await collect(one.fetchPage, 50);
  assert.equal(first.items.length, 50);
  assert.deepEqual(first.next, { key: 0, id: 50 });
  assert.equal(one.calls.length, 1);

  const two = server(330);
  const again = await collect(two.fetchPage, 260);
  assert.equal(again.items.length, 260);
  assert.deepEqual(two.calls.map((c) => c.limit), [200, 60]);
  assert.deepEqual(again.next, { key: 260, id: 260 });

  const three = server(30);
  const short = await collect(three.fetchPage, 120);
  assert.equal(short.items.length, 30);
  assert.equal(short.next, null);
  assert.equal(short.last.counts.waiting, 30);
});

test('фильтр и срок в днях', () => {
  assert.equal(readFilter(null), 'waiting');
  assert.equal(readFilter('closed'), 'closed');
  assert.equal(readFilter('всё'), 'waiting');
  assert.equal(readDays('30'), 30);
  assert.equal(readDays(' 7 '), 7);
  assert.equal(readDays('0'), null);
  assert.equal(readDays('3651'), null);
  assert.equal(readDays('3,5'), null);
  assert.equal(readDays(''), null);
});
