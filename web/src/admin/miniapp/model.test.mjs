import { registerHooks } from 'node:module';
import { test } from 'node:test';
import assert from 'node:assert/strict';

registerHooks({
  resolve(specifier, context, next) {
    if (specifier.startsWith('.') && !/\.[a-z]+$/.test(specifier)) {
      return next(`${specifier}.ts`, context);
    }
    return next(specifier, context);
  },
});

const model = await import('./model.ts');
const { normalizeConfig, standardConfig, standardHome } = await import('../../mini/config.ts');
const { LIMITS } = model;

const home = (block) => normalizeConfig({ home: [block] });
const texts = (config) => model.problemsOf(config, LIMITS).map((problem) => problem.text);
const standard = (key) => ({ renew: 'Продлить', devices: 'Устройства', friends: 'Друзья' })[key] ?? key;

test('стандартная настройка не считается изменённой и отправляется без лишнего', () => {
  const config = standardConfig();
  assert.ok(model.isStandard(config));
  assert.ok(model.sameConfig(config, normalizeConfig({})));
  assert.deepEqual(model.problemsOf(config, LIMITS), []);
  const out = model.canonical(config);
  assert.deepEqual(out.header, { balance: true, avatar: false });
  assert.deepEqual(out.home[0], { kind: 'banners' });
  assert.deepEqual(out.home[1], { kind: 'greeting', on: false });
  assert.deepEqual(out.home[2], { kind: 'subscription' });
  assert.deepEqual(out.home[3], { kind: 'actions', items: [{ std: 'renew' }, { std: 'devices' }] });
  assert.deepEqual(out.tabs, [{ std: 'home' }, { std: 'plans' }, { std: 'connect' }, { std: 'profile' }]);
  assert.deepEqual(out.profile[0], { kind: 'person', on: false });
  assert.deepEqual(out.profile[2], { kind: 'friends' });
  assert.deepEqual(out.profile[3], { kind: 'help' });
  for (const block of config.home) {
    assert.ok(!model.homeChanged(block), block.kind);
  }
  for (const block of config.profile) {
    assert.ok(!model.profileChanged(block), block.kind);
  }
});

test('в запрос уходят только заданные поля', () => {
  const config = normalizeConfig({
    home: [
      {
        kind: 'banners',
        items: [
          {
            on: true,
            image: { tag: 'abc', kind: 'png', width: 10, height: 5, url: '/x' },
            title: { ru: '  Скидка ', en: '' },
            text: { ru: '' },
            to: ' plans ',
            for: ['new', 'trial', 'active', 'ended'],
          },
        ],
      },
      { kind: 'subscription', look: 'ring', empty: { title: { ru: '' }, line: { ru: '' } } },
      { kind: 'sections', items: [{ std: 'gifts', on: false, label: { ru: ' ' } }, { label: { ru: 'Канал' }, to: 'https://t.me/x', for: ['ended', 'new'] }] },
    ],
    tabs: [{ std: 'home', label: {} }, { std: 'plans', label: { ru: 'Купить' } }, { std: 'profile' }],
    profile: [{ kind: 'friends', place: 'card', text: { ru: '' } }],
  });
  const out = model.canonical(config);
  const block = (kind) => out.home.find((item) => item.kind === kind);
  assert.deepEqual(block('banners').items, [{ image: { tag: 'abc' }, title: { ru: 'Скидка' }, to: 'plans' }]);
  assert.deepEqual(block('subscription'), { kind: 'subscription', empty: { line: { ru: '' } } });
  const sections = block('sections').items;
  assert.deepEqual(
    sections.find((item) => item.std === 'gifts'),
    { std: 'gifts', on: false },
  );
  assert.deepEqual(
    sections.find((item) => !item.std),
    { label: { ru: 'Канал' }, to: 'https://t.me/x', for: ['new', 'ended'] },
  );
  assert.deepEqual(out.tabs, [{ std: 'home' }, { std: 'plans', label: { ru: 'Купить' } }, { std: 'profile' }]);
  assert.deepEqual(
    out.profile.find((item) => item.kind === 'friends'),
    { kind: 'friends', text: { ru: '' } },
  );
});

test('порядок меняется перетаскиванием и стрелками', () => {
  const list = ['a', 'b', 'c', 'd'];
  assert.deepEqual(model.moved(list, 0, 2), ['b', 'c', 'a', 'd']);
  assert.deepEqual(model.moved(list, 3, 0), ['d', 'a', 'b', 'c']);
  assert.deepEqual(model.moved(list, 1, 9), ['a', 'c', 'd', 'b']);
  assert.equal(model.moved(list, 9, 0), list);
  assert.equal(model.moved(list, 2, 2), list);
  assert.equal(model.dropIndex(0, 2, false), 1);
  assert.equal(model.dropIndex(0, 2, true), 2);
  assert.equal(model.dropIndex(3, 1, false), 1);
  assert.equal(model.dropIndex(3, 1, true), 2);
  assert.equal(model.dropIndex(2, 2, true), 2);
});

test('перестановка блока — это изменение, возврат на место — нет', () => {
  const config = standardConfig();
  const next = { ...config, home: model.moved(config.home, 4, 2) };
  assert.ok(!model.sameConfig(config, next));
  assert.ok(model.sameConfig(config, { ...next, home: model.moved(next.home, 2, 4) }));
});

test('правка текста и возврат к стандартному', () => {
  assert.deepEqual(model.setPhrase(undefined, 'ru', 'Купить'), { ru: 'Купить' });
  assert.deepEqual(model.setPhrase({ ru: 'Купить', en: 'Buy' }, 'en', null), { ru: 'Купить' });
  assert.equal(model.setPhrase({ ru: 'Купить' }, 'ru', null), undefined);
  const changed = { kind: 'subscription', look: 'compact' };
  assert.ok(model.homeChanged(changed));
  assert.deepEqual(model.resetHome(changed), { kind: 'subscription' });
  assert.deepEqual(model.resetHome({ kind: 'actions', on: false, items: [] }), { ...standardHome('actions'), on: false });
  assert.deepEqual(model.resetProfile({ kind: 'friends', on: false, place: 'screen' }), { kind: 'friends', on: false });
  assert.deepEqual(model.resetProfile({ kind: 'help' }), { kind: 'help' });
  assert.ok(model.profileChanged({ kind: 'help', gifts: true }));
  assert.deepEqual(model.resetProfile({ kind: 'help', gifts: true }), { kind: 'help' });
});

test('скрытие блока само по себе не «свой вид блока»', () => {
  assert.ok(!model.homeChanged({ ...standardHome('sections'), on: false }));
  assert.ok(!model.profileChanged({ kind: 'balance', on: false }));
  assert.ok(!model.profileChanged({ kind: 'person', on: true }));
});

test('ссылки: разделы, https и t.me', () => {
  assert.equal(model.targetProblem('plans', LIMITS.url), null);
  assert.equal(model.targetProblem('https://example.com/a?b=1', LIMITS.url), null);
  assert.equal(model.targetProblem('https://пример.рф/скидка', LIMITS.url), null);
  assert.equal(model.normalizeUrl(' t.me/news '), 'https://t.me/news');
  assert.equal(model.urlProblem('t.me/news', LIMITS.url), null);
  assert.equal(model.targetProblem('', LIMITS.url), 'укажите, куда ведёт');
  assert.equal(model.targetProblem(undefined, LIMITS.url), 'укажите, куда ведёт');
  for (const bad of ['http://example.com', 'javascript:alert(1)', 'data:text/html,x', 'tg://x', 'example.com', '/app/plans']) {
    assert.equal(model.urlProblem(bad, LIMITS.url), 'нужна ссылка вида https://… или t.me/…', bad);
  }
  for (const bad of ['https://a b.com', 'https://x.com/"><script>', "https://x.com/'x", 'https://x.com/<b>', 'https://x.com\\@evil.com']) {
    assert.equal(model.urlProblem(bad, LIMITS.url), 'в ссылке не должно быть пробелов, кавычек и скобок', bad);
  }
  for (const bad of ['https://', 'https://user@example.com', 'https://.example.com', 'https://example.com-', 'https://example.com:порт']) {
    assert.equal(model.urlProblem(bad, LIMITS.url), 'в ссылке нет адреса сайта', bad);
  }
  assert.equal(model.urlProblem(`https://x.com/${'a'.repeat(500)}`, LIMITS.url), 'не длиннее 500 символов');
});

test('ошибки названы по-русски с местом', () => {
  assert.deepEqual(texts(home({ kind: 'banners', items: [{ text: { ru: 'Без заголовка' } }] })), ['Главная, баннер 1: нужна картинка или заголовок']);
  assert.deepEqual(texts(home({ kind: 'banners', items: [{ title: { ru: 'Скидка' }, to: '' }] })), ['Главная, баннер 1: ссылка: укажите, куда ведёт']);
  assert.deepEqual(texts(home({ kind: 'banners', items: [{ title: { ru: 'Скидка' } }] })), []);
  assert.deepEqual(texts(home({ kind: 'greeting', on: true })), ['Главная, приветствие: нужен заголовок или текст']);
  assert.deepEqual(texts(home({ kind: 'greeting', on: false })), []);
  assert.deepEqual(texts(home({ kind: 'greeting', on: true, title: { ru: 'Ж'.repeat(61) } })), ['Главная, приветствие, заголовок: не длиннее 60 символов, сейчас 61']);
  assert.deepEqual(texts(home({ kind: 'sections', items: [{ label: { ru: ' ' }, to: 'plans' }] })), ['Главная, разделы, ссылка без названия: нужно название']);
  assert.deepEqual(texts(home({ kind: 'sections', items: [{ label: { ru: 'Канал' }, to: 'http://x.com' }] })), ['Главная, разделы, «Канал»: ссылка: нужна ссылка вида https://… или t.me/…']);
  assert.deepEqual(texts(home({ kind: 'actions', items: [{ std: 'renew', label: { en: 'a'.repeat(25) } }] })), ['Главная, кнопки, «Продлить»: на английском не длиннее 24 символов, сейчас 25']);
  assert.deepEqual(texts(normalizeConfig({ tabs: [{ std: 'home' }, { std: 'plans', label: { ru: 'Ж'.repeat(13) } }, { std: 'profile' }] })), ['Меню внизу, «Тарифы»: не длиннее 12 символов, сейчас 13']);
  assert.deepEqual(texts(normalizeConfig({ profile: [{ kind: 'links', items: [{ label: { ru: 'Канал' } }] }] })), ['Профиль, свои ссылки, «Канал»: ссылка: укажите, куда ведёт']);
});

test('у каждой части экрана своя вкладка конструктора', () => {
  assert.equal(model.screenOf('header'), 'home');
  assert.equal(model.screenOf('banners'), 'home');
  assert.equal(model.screenOf('sections'), 'home');
  assert.equal(model.screenOf('tabs'), 'tabs');
  for (const part of ['balance', 'friends', 'help', 'links', 'documents', 'account']) {
    assert.equal(model.screenOf(part), 'profile', part);
  }
});

test('подписи блоков показывают, что внутри', () => {
  const summary = (block) => model.homeSummary(home(block).home.find((item) => item.kind === block.kind), standard).replace(/\u00a0/g, ' ');
  assert.equal(summary({ kind: 'banners' }), 'Пока нет');
  assert.equal(summary({ kind: 'banners', items: [{ title: { ru: 'Раз' } }, { title: { ru: 'Два' }, on: false }] }), '2 баннера, скрыто 1');
  assert.equal(summary({ kind: 'greeting', on: false, title: { ru: 'Привет' } }), 'Скрыто');
  assert.equal(summary({ kind: 'greeting', on: true, text: { ru: 'Добрый день' } }), 'Добрый день');
  assert.equal(summary({ kind: 'actions' }), 'Продлить, Устройства');
  assert.equal(summary({ kind: 'actions', items: [] }), 'Без кнопок');
  assert.equal(summary({ kind: 'actions', items: [{ std: 'renew', label: { ru: 'Оплатить' } }, { label: {}, to: 'plans' }] }), 'Оплатить, Без названия');
  assert.equal(summary({ kind: 'sections', items: [{ std: 'gifts', on: false }] }), '8 пунктов, скрыто 1');
  assert.equal(model.profileSummary({ kind: 'friends', place: 'screen' }), 'Отдельный экран');
  assert.equal(model.profileSummary({ kind: 'balance', on: false }), 'Видят только клиенты с деньгами');
  assert.equal(model.profileSummary({ kind: 'person' }), 'Скрыто');
  assert.equal(model.profileSummary({ kind: 'person', on: true }), 'Фото, имя и @username из Telegram');
  assert.equal(model.profileSummary({ kind: 'help', gifts: true }), 'Подарок, история оплат и поддержка');
  assert.equal(model.audienceText(['ended', 'new']), 'новые, закончилась');
  assert.equal(model.audienceText(undefined), 'всем');
  assert.equal(model.linkText('plans'), 'Раздел «Тарифы»');
  assert.equal(model.linkText('https://t.me/news'), 't.me/news');
  assert.ok(model.tabFixed('plans'));
  assert.ok(!model.tabFixed('connect'));
});
