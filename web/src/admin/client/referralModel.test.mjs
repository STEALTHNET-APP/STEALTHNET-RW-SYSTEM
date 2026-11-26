import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  askSetText,
  candidateLine,
  emptyText,
  hidden,
  invitedLine,
  inviterLine,
  inviterWarning,
  leadText,
  moreText,
  personContact,
  personTitle,
  referralCode,
  referralHistory,
  refusalText,
} from './referralModel.ts';

const fmt = {
  money: (m) => `${m.minor / 100} ₽`,
  date: (iso) => {
    const d = new Date(iso);
    const months = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];
    return `${d.getUTCDate()} ${months[d.getUTCMonth()]}`;
  },
};

const rub = (minor) => ({ minor, currency: 'RUB' });
const alice = { client_id: 12, name: 'Елена Воронова', telegram_username: 'lena_vo', email: null };

function section(overrides = {}) {
  return {
    module_enabled: true,
    inviter: null,
    invited_count: 14,
    money: { invited_paid: 6, earned: rub(325000), withdrawn: rub(200000), pending: rub(50000), balance: rub(75000) },
    invited: { items: [], next: 41 },
    ...overrides,
  };
}

test('люди подписываются как в остальной админке: имя, id Telegram, почта, номер', () => {
  assert.equal(personTitle(alice), 'Елена Воронова');
  assert.equal(personContact(alice), '@lena_vo');
  const nick = { client_id: 3, name: null, telegram_username: 'nick', telegram_id: '7412', email: 'a@b.ru' };
  assert.equal(personTitle(nick), 'id 7412');
  assert.equal(personContact(nick), '@nick');
  assert.equal(personContact({ ...nick, telegram_username: null }), 'a@b.ru');
  const mail = { client_id: 4, name: ' ', telegram_username: null, telegram_id: null, email: 'mari@li.com' };
  assert.equal(personTitle(mail), 'mari@li.com');
  assert.equal(personContact(mail), null);
  assert.equal(personTitle({ client_id: 5, name: null, telegram_username: null }), 'Клиент 5');
});

test('строка пригласившего говорит когда, как и по какому коду', () => {
  const natural = { client: alice, blocked: false, deleted: false, linked_at: '2026-11-12T09:15:00Z', source: 'bot', code: 'K7M2QX', manual: null };
  assert.equal(inviterLine(natural, fmt), '12 ноября, по ссылке в боте, код K7M2QX');
  assert.equal(inviterLine({ ...natural, source: 'web', code: 'K7M2QX' }, fmt), '12 ноября, по ссылке на сайт, код K7M2QX');
  assert.equal(inviterLine({ ...natural, source: 'import', code: null }, fmt), '12 ноября, перенос из прежней панели');
  assert.equal(inviterLine({ ...natural, source: 'admin', code: null }, fmt), '12 ноября');
  const manual = { ...natural, manual: { at: '2026-11-26T10:00:00Z', by: 'owner@example.com' } };
  assert.equal(inviterLine(manual, fmt), 'привязка вручную 26 ноября, owner@example.com');
  assert.equal(inviterLine({ ...manual, manual: { at: '2026-11-26T10:00:00Z', by: null } }, fmt), 'привязка вручную 26 ноября');
  assert.equal(inviterWarning(natural), null);
  assert.match(inviterWarning({ ...natural, blocked: true }), /заблокирован/);
  assert.match(inviterWarning({ ...natural, deleted: true, blocked: true }), /удалён/);
});

test('строка приглашённого: дата прихода, оплаты и ручная привязка', () => {
  const row = { client: alice, joined_at: '2026-11-20T08:00:00Z', manual_at: null, blocked: false, deleted: false, paid: false, bonus: rub(0) };
  assert.equal(invitedLine(row, fmt), 'с 20 ноября · оплат нет');
  assert.equal(invitedLine({ ...row, joined_at: '2026-11-02T08:00:00Z', paid: true }, fmt), 'со 2 ноября · есть оплаты');
  assert.equal(invitedLine({ ...row, paid: true, manual_at: '2026-11-21T08:00:00Z' }, fmt), 'с 20 ноября · есть оплаты · привязка вручную 21 ноября');
  const noMoney = { client: alice, joined_at: '2026-11-20T08:00:00Z', manual_at: null, blocked: true, deleted: false };
  assert.equal(invitedLine(noMoney, fmt), 'с 20 ноября · заблокирован');
  assert.equal(invitedLine({ ...noMoney, deleted: true }, fmt), 'с 20 ноября · удалён');
});

test('итоги одной фразой и без денег для тех, у кого нет права', () => {
  assert.equal(
    leadText(section(), fmt),
    '14 приглашённых, у 6 есть оплаты. Бонусов начислено 3250 ₽: выведено 2000 ₽, ждёт выплаты 500 ₽, на реферальном балансе 750 ₽.',
  );
  assert.equal(leadText(section({ invited_count: 1, money: undefined }), fmt), '1 приглашённый.');
  assert.equal(leadText(section({ invited_count: 3, money: undefined }), fmt), '3 приглашённых.');
  const quiet = section({ invited_count: 2, money: { invited_paid: 0, earned: rub(0), withdrawn: rub(0), pending: rub(0), balance: rub(0) } });
  assert.equal(leadText(quiet, fmt), '2 приглашённых, оплат у них пока нет.');
  const nobody = section({ invited_count: 0, money: { invited_paid: 0, earned: rub(0), withdrawn: rub(0), pending: rub(0), balance: rub(12000) } });
  assert.equal(leadText(nobody, fmt), 'Приглашённых пока нет. На реферальном балансе 120 ₽.');
  assert.equal(leadText(section({ money: { invited_paid: 6, earned: rub(100000), withdrawn: rub(0), pending: rub(0), balance: rub(100000) } }), fmt), '14 приглашённых, у 6 есть оплаты. Бонусов начислено 1000 ₽: на реферальном балансе 1000 ₽.');
});

test('пустой раздел сжимается, а выключенный модуль без данных прячет раздел', () => {
  assert.equal(emptyText(section({ invited_count: 0 })), 'Пригласившего нет, приглашённых нет.');
  assert.equal(emptyText(section()), null);
  assert.equal(hidden(section({ module_enabled: false, invited_count: 0 })), true);
  assert.equal(hidden(section({ module_enabled: false })), false);
  assert.equal(hidden(section({ invited_count: 0 })), false);
});

test('показать ещё считает остаток', () => {
  assert.equal(moreText(section(), 5), 'Показать ещё 9');
  assert.equal(moreText(section({ invited: { items: [], next: null } }), 5), null);
  assert.equal(moreText(section({ invited_count: 5 }), 5), 'Показать ещё');
});

test('кандидаты объясняют, почему их нельзя выбрать', () => {
  const candidate = { client: alice, joined_at: '2026-09-03T00:00:00Z', blocked: false, refusal: null };
  assert.equal(candidateLine(candidate, fmt), '№12, с нами с 3 сентября');
  assert.equal(candidateLine({ ...candidate, blocked: true }, fmt), '№12, с нами с 3 сентября, заблокирован, бонусы не начисляются');
  assert.equal(candidateLine({ ...candidate, refusal: 'cycle' }, fmt), 'Нельзя: в числе приглашённых этого клиента, получится круг');
  assert.equal(refusalText('itself'), 'Это сам клиент');
  assert.equal(refusalText('current'), 'Уже пригласивший');
  assert.equal(refusalText(null), null);
  const ask = askSetText(candidate, true);
  assert.equal(ask.head, 'Пригласившим станет Елена Воронова.');
  assert.match(ask.tail, /не пересчитываются/);
  assert.doesNotMatch(askSetText(candidate, false).tail, /пересчитываются/);
});

test('код приглашения достаётся из параметра старта', () => {
  assert.equal(referralCode('ref_K7M2QX'), 'K7M2QX');
  assert.equal(referralCode('s_tiktok__ref_k7m2qx'), 'K7M2QX');
  assert.equal(referralCode('auth_0123abcd__ref_AB12'), 'AB12');
  assert.equal(referralCode('ref_bad-code'), null);
  assert.equal(referralCode('s_tiktok'), null);
  assert.equal(referralCode(null), null);
});

test('история показывает смену пригласившего каждой стороне по-своему', () => {
  const set = {
    action: 'client.referrer_set',
    entity_id: 41,
    details: {
      from: { referrer: 'Елена Воронова · @lena_vo · №12' },
      to: { referrer: 'Борис Ким · @boris_k · №15' },
      invitee: 'Павел Рыжов · @pavel_rz · №41',
      referrer_id: 15,
      previous_referrer_id: 12,
    },
  };
  assert.deepEqual(referralHistory(set, 41, 'owner@example.com'), {
    what: 'Сменён пригласивший',
    note: 'owner@example.com: было Елена Воронова · @lena_vo · №12, стало Борис Ким · @boris_k · №15',
  });
  assert.deepEqual(referralHistory(set, 15, 'owner@example.com'), {
    what: 'Добавлен приглашённый',
    note: 'owner@example.com: Павел Рыжов · @pavel_rz · №41, раньше Елена Воронова · @lena_vo · №12',
  });
  assert.deepEqual(referralHistory(set, 12, 'owner@example.com'), {
    what: 'Отвязан приглашённый',
    note: 'owner@example.com: Павел Рыжов · @pavel_rz · №41, теперь Борис Ким · @boris_k · №15',
  });
  const first = { ...set, details: { ...set.details, from: { referrer: null }, previous_referrer_id: null } };
  assert.equal(referralHistory(first, 41, 'a').what, 'Назначен пригласивший');
  assert.equal(referralHistory(first, 15, 'a').note, 'a: Павел Рыжов · @pavel_rz · №41');
  const removed = {
    action: 'client.referrer_removed',
    entity_id: 41,
    details: { from: { referrer: 'Борис Ким · @boris_k · №15' }, to: { referrer: null }, invitee: 'Павел Рыжов · @pavel_rz · №41', previous_referrer_id: 15 },
  };
  assert.deepEqual(referralHistory(removed, 41, 'a'), { what: 'Убран пригласивший', note: 'a: было Борис Ким · @boris_k · №15' });
  assert.deepEqual(referralHistory(removed, 15, 'a'), { what: 'Отвязан приглашённый', note: 'a: Павел Рыжов · @pavel_rz · №41' });
  assert.equal(referralHistory(removed, 99, 'a'), null);
  assert.equal(referralHistory({ action: 'client.note', entity_id: 41, details: {} }, 41, 'a'), null);
});
