import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  arrangeRows,
  audienceOf,
  currentTab,
  friendsPath,
  homeBlock,
  homeOn,
  imageOf,
  imageUrl,
  isPart,
  linkIcon,
  linkUrl,
  normalizeConfig,
  pick,
  profileBlock,
  profileOn,
  say,
  shownTo,
  standardConfig,
  withName,
  withPercent,
} from './config.ts';

const kinds = (blocks) => blocks.map((block) => block.kind);
const keys = (tabs) => tabs.map((tab) => tab.std);
const rowNames = (items) => items.map((item) => item.std ?? `own:${item.label.ru}`);

test('без настроек мини-приложение выглядит как раньше', () => {
  for (const raw of [undefined, null, {}, [], 'мусор', 5]) {
    assert.deepEqual(normalizeConfig(raw), standardConfig());
  }
  const config = standardConfig();
  assert.deepEqual(kinds(config.home), ['banners', 'greeting', 'subscription', 'actions', 'sections']);
  assert.deepEqual(kinds(config.home.filter(homeOn)), ['banners', 'subscription', 'actions', 'sections']);
  assert.deepEqual(keys(config.tabs), ['home', 'plans', 'connect', 'profile']);
  assert.deepEqual(kinds(config.profile), ['person', 'balance', 'friends', 'help', 'links', 'documents', 'account']);
  assert.deepEqual(rowNames(homeBlock(config, 'sections').items), ['friends', 'roulette', 'addons', 'gifts', 'promo', 'payments', 'guide', 'support']);
  assert.deepEqual(homeBlock(config, 'actions').items, [{ std: 'renew' }, { std: 'devices' }]);
  assert.equal(config.header.balance, true);
  assert.equal(config.header.avatar, false);
  assert.equal(friendsPath(config), '/app/profile');
});

test('порядок и показ блоков главной берутся из настроек', () => {
  const config = normalizeConfig({
    header: { balance: false, avatar: true },
    home: [
      { kind: 'sections', on: false },
      { kind: 'greeting', on: true, title: { ru: 'Привет' } },
      { kind: 'subscription', on: false },
      { kind: 'video' },
      { kind: 'greeting', on: false },
      null,
    ],
  });
  assert.equal(config.header.balance, false);
  assert.equal(config.header.avatar, true);
  assert.deepEqual(kinds(config.home), ['banners', 'sections', 'greeting', 'subscription', 'actions']);
  assert.deepEqual(kinds(config.home.filter(homeOn)), ['banners', 'greeting', 'subscription', 'actions']);
  assert.equal(homeBlock(config, 'greeting').title.ru, 'Привет');
});

test('подписку нельзя спрятать, а недостающие блоки встают на место', () => {
  const config = normalizeConfig({ home: [{ kind: 'actions', items: [] }, { kind: 'subscription', on: false }] });
  assert.ok(homeOn(homeBlock(config, 'subscription')));
  assert.deepEqual(homeBlock(config, 'actions').items, []);
  assert.deepEqual(kinds(config.home), ['banners', 'greeting', 'actions', 'sections', 'subscription']);
});

test('новый раздел из кода встаёт на своё место, чужой пропускается', () => {
  const rows = arrangeRows([
    { std: 'support' },
    { label: { ru: 'Канал' }, to: 'https://t.me/news' },
    { std: 'friends', on: false },
    { std: 'support' },
    { std: 'renew' },
    { std: 'неизвестно' },
    null,
  ]);
  assert.deepEqual(rowNames(rows), ['support', 'own:Канал', 'friends', 'roulette', 'addons', 'gifts', 'promo', 'payments', 'guide']);
  assert.equal(rows[2].on, false);
});

test('кнопка, которой нет в списке, и повтор пропускаются', () => {
  const config = normalizeConfig({
    home: [
      {
        kind: 'actions',
        items: [{ std: 'gifts' }, { std: 'plans' }, { std: 'неизвестно' }, { std: 'gifts' }, { label: { ru: 'Канал' }, to: 'https://t.me/news' }, null],
      },
    ],
  });
  assert.deepEqual(rowNames(homeBlock(config, 'actions').items), ['gifts', 'own:Канал']);
});

test('без обязательных вкладок меню остаётся стандартным', () => {
  const tabs = (list) => keys(normalizeConfig({ tabs: list }).tabs);
  assert.deepEqual(tabs([{ std: 'profile' }, { std: 'plans' }, { std: 'home' }]), ['profile', 'plans', 'home']);
  assert.deepEqual(tabs([{ std: 'home' }, { std: 'plans' }, { std: 'friends' }, { std: 'support' }, { std: 'profile' }]), ['home', 'plans', 'friends', 'support', 'profile']);
  const standard = ['home', 'plans', 'connect', 'profile'];
  assert.deepEqual(tabs([]), standard);
  assert.deepEqual(tabs([{ std: 'home' }, { std: 'profile' }]), standard);
  assert.deepEqual(tabs([{ std: 'home' }, { std: 'plans' }, { std: 'profile' }, { std: 'trial' }]), ['home', 'plans', 'profile']);
  assert.deepEqual(tabs([{ std: 'home' }, { std: 'plans' }, { std: 'profile' }, { std: 'friends' }, { std: 'gifts' }, { std: 'promo' }]), standard);
});

test('профиль: обязательные блоки видны всегда', () => {
  const config = normalizeConfig({
    profile: [
      { kind: 'account', on: false },
      { kind: 'help', on: false },
      { kind: 'balance', on: false },
      { kind: 'links', items: [{ std: 'promo' }, { label: { ru: 'Сайт' }, to: 'https://example.com' }] },
    ],
  });
  assert.deepEqual(kinds(config.profile), ['person', 'account', 'help', 'balance', 'friends', 'links', 'documents']);
  assert.deepEqual(kinds(config.profile.filter(profileOn)), ['account', 'help', 'friends', 'links', 'documents']);
  assert.deepEqual(profileBlock(config, 'links').items, [{ label: { ru: 'Сайт' }, to: 'https://example.com' }]);
});

test('профиль: фото с именем и подарок видны только включёнными', () => {
  const old = normalizeConfig({ profile: [{ kind: 'account' }, { kind: 'person' }] });
  assert.equal(profileOn(profileBlock(old, 'person')), false);
  assert.equal(profileBlock(old, 'help').gifts, undefined);
  assert.equal(profileOn(profileBlock(normalizeConfig({ profile: [{ kind: 'person', on: 'да' }] }), 'person')), false);
  const on = normalizeConfig({ profile: [{ kind: 'person', on: true }, { kind: 'help', gifts: true }] });
  assert.equal(profileOn(profileBlock(on, 'person')), true);
  assert.equal(profileBlock(on, 'help').gifts, true);
});

test('«Друзья» ведут в профиль, на свой экран или остаются без карточки', () => {
  const path = (block) => friendsPath(normalizeConfig({ profile: [{ kind: 'friends', ...block }] }));
  assert.equal(path({}), '/app/profile');
  assert.equal(path({ place: 'screen' }), '/app/friends');
  assert.equal(path({ on: false }), '/app/friends');
});

test('текст берётся по языку клиента', () => {
  assert.equal(say({ ru: 'Скидка', en: 'Sale' }, 'en'), 'Sale');
  assert.equal(say({ ru: 'Скидка' }, 'en'), 'Скидка');
  assert.equal(say({ ru: 'Скидка', en: '' }, 'en'), 'Скидка');
  assert.equal(say({ en: 'Sale' }, 'ru'), '');
  assert.equal(say(undefined, 'ru'), '');
  assert.equal(pick({ ru: 'Купить' }, 'ru', 'Тарифы'), 'Купить');
  assert.equal(pick({ ru: 'Купить' }, 'en', 'Plans'), 'Plans');
  assert.equal(pick({ ru: '' }, 'ru', 'Подписки пока нет'), '');
  assert.equal(pick(null, 'ru', 'Тарифы'), 'Тарифы');
});

test('имя подставляется, а без имени текст остаётся связным', () => {
  assert.equal(withName('Привет, {name}!', 'Алексей'), 'Привет, Алексей!');
  assert.equal(withName('Привет, {name}!', null), 'Привет!');
  assert.equal(withName('{name}, рады видеть вас', null), 'Рады видеть вас');
  assert.equal(withName('{name}!', null), '');
  assert.equal(withName('Добрый день', null), 'Добрый день');
  assert.equal(withName('Привет, {name}! {name}, загляните в тарифы', null), 'Привет!, загляните в тарифы');
  assert.equal(withPercent('Вы получаете {percent}% и ещё {percent}%', 30), 'Вы получаете 30% и ещё 30%');
});

test('аудитория считается по всем подпискам клиента', () => {
  const sub = (status, kind = 'paid') => ({ status, subscription: { kind } });
  assert.equal(audienceOf([]), 'new');
  assert.equal(audienceOf([sub('expired')]), 'ended');
  assert.equal(audienceOf([sub('expired', 'trial')]), 'ended');
  assert.equal(audienceOf([sub('trial', 'trial')]), 'trial');
  assert.equal(audienceOf([sub('expired'), sub('trial', 'trial')]), 'trial');
  assert.equal(audienceOf([sub('expired'), sub('limited')]), 'active');
  assert.equal(audienceOf([sub('trial', 'trial'), sub('active', 'gift')]), 'active');
  assert.ok(shownTo(undefined, 'new'));
  assert.ok(shownTo([], 'ended'));
  assert.ok(shownTo(['new', 'ended'], 'ended'));
  assert.ok(!shownTo(['new'], 'active'));
});

test('ссылки открываются только по https', () => {
  assert.equal(linkUrl('https://example.com/a?b=1#c'), 'https://example.com/a?b=1#c');
  for (const bad of ['http://example.com', 'javascript:alert(1)', 'data:text/html,x', 'tg://resolve', '//example.com', '/app', 'https://a b', 'https://x"y', "https://x'y", 'https://x<y', 'https://', '', null, undefined]) {
    assert.equal(linkUrl(bad), null, String(bad));
  }
});

test('значок своей ссылки выбирается сам, если его не задали', () => {
  assert.equal(linkIcon({ to: 'https://t.me/news' }), 'send');
  assert.equal(linkIcon({ to: 'https://example.com' }), 'globe');
  assert.equal(linkIcon({ to: 'roulette' }), 'trophy');
  assert.equal(linkIcon({ to: 'https://t.me/news', icon: 'flag' }), 'flag');
});

test('картинка баннера по языку, без английской — русская', () => {
  const ru = { tag: 'a', kind: 'png', url: '/ru.png' };
  const en = { tag: 'b', kind: 'jpeg' };
  assert.equal(imageOf({ image: ru, image_en: en }, 'ru'), ru);
  assert.equal(imageOf({ image: ru, image_en: en }, 'en'), en);
  assert.equal(imageOf({ image: ru }, 'en'), ru);
  assert.equal(imageOf({ image_en: en }, 'ru'), null);
  assert.equal(imageUrl(ru), '/ru.png');
  assert.equal(imageUrl(en), '/api/public/miniapp/b.jpg');
  assert.equal(imageUrl(null), null);
});

test('подсветка вкладки как раньше, свои вкладки подсвечиваются на своём экране', () => {
  const standard = standardConfig().tabs;
  for (const path of ['/app', '/app/', '/app/topup', '/app/payments', '/app/roulette', '/app/promo']) {
    assert.equal(currentTab(standard, path), 'home', path);
  }
  assert.equal(currentTab(standard, '/app/plans'), 'plans');
  assert.equal(currentTab(standard, '/app/connect'), 'connect');
  assert.equal(currentTab(standard, '/app/profile/email'), 'profile');
  assert.equal(currentTab(standard, '/app/friends'), 'profile');
  for (const path of ['/app/gifts', '/app/addons', '/app/trial', '/app/support/3', '/app/withdraw']) {
    assert.equal(currentTab(standard, path), null, path);
  }
  const own = [{ std: 'home' }, { std: 'plans' }, { std: 'roulette' }, { std: 'friends' }, { std: 'profile' }];
  assert.equal(currentTab(own, '/app/roulette'), 'roulette');
  assert.equal(currentTab(own, '/app/friends'), 'friends');
  assert.equal(currentTab(own, '/app/promo'), 'home');
});

test('из превью принимаются только известные части экрана', () => {
  assert.ok(isPart('tabs'));
  assert.ok(isPart('sections'));
  assert.ok(isPart('account'));
  for (const bad of ['', 'script', '__proto__', null, 5, {}]) {
    assert.ok(!isPart(bad));
  }
});
