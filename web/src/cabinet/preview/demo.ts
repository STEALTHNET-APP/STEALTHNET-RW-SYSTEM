import { ApiError, type Answer } from '../../lib/api';
import type { Money } from '../../lib/format';
import type { Lang } from '../../lib/i18n';
import type { ChatThread } from '../../lib/supportChat';
import type { Site } from '../../lib/types';
import { ALL_PLATFORMS, type ClientApp, type ClientApps } from '../connect';
import type { AddonsOffer, ItemQuote } from '../extras';
import type { GiftOffers, MyGift } from '../gifting';
import type { Catalog, CheckoutMethod, ClientMe, Device, PeriodOffer, Quote, QuoteLineView, Referral, SubscriptionView, TariffOffer } from '../model';
import type { ClientPayment, TopUpOptions } from '../payments';
import type { TrialOffer } from '../trial';
import type { PreviewState } from './model';

const DAY = 86_400_000;
const HOUR = 3_600_000;

type Names = { standard: string; standardText: string; max: string; maxText: string; card: string; cardHint: string; crypto: string; cryptoHint: string; balance: string; balanceHint: string; subject: string; question: string; reply: string; tariffs: string };

const NAMES: Record<Lang, Names> = {
  ru: {
    standard: 'Стандарт',
    standardText: 'Для большинства задач',
    max: 'Максимум',
    maxText: 'Больше устройств на одну подписку',
    card: 'Банковская карта',
    cardHint: 'Оплата на странице банка',
    crypto: 'Криптовалюта',
    cryptoHint: 'USDT, TON, BTC',
    balance: 'С баланса',
    balanceHint: 'Списывается вся сумма сразу',
    subject: 'Как подключить телевизор',
    question: 'Как подключить телевизор?',
    reply: 'Установите приложение на телевизор и отсканируйте QR-код из кабинета.',
    tariffs: 'Тарифы',
  },
  en: {
    standard: 'Standard',
    standardText: 'For most tasks',
    max: 'Max',
    maxText: 'More devices on one subscription',
    card: 'Bank card',
    cardHint: 'Payment on the bank page',
    crypto: 'Cryptocurrency',
    cryptoHint: 'USDT, TON, BTC',
    balance: 'From balance',
    balanceHint: 'The whole amount is charged at once',
    subject: 'How to connect a TV',
    question: 'How do I connect a TV?',
    reply: 'Install the app on the TV and scan the QR code from your account.',
    tariffs: 'Plans',
  },
};

const ago = (ms: number) => new Date(Date.now() - ms).toISOString();

type Terms = { id: number; months: number; days: number; usd: number; rub: number; stars: number; savings: number | null };

const STANDARD: Terms[] = [
  { id: 1, months: 1, days: 30, usd: 299, rub: 19_900, stars: 150, savings: null },
  { id: 2, months: 3, days: 90, usd: 699, rub: 49_900, stars: 350, savings: 22 },
  { id: 3, months: 12, days: 365, usd: 2399, rub: 169_000, stars: 1200, savings: 33 },
];

const MAX: Terms[] = [
  { id: 4, months: 1, days: 30, usd: 499, rub: 34_900, stars: 250, savings: null },
  { id: 5, months: 3, days: 90, usd: 1299, rub: 89_900, stars: 650, savings: 13 },
  { id: 6, months: 12, days: 365, usd: 4199, rub: 299_000, stars: 2100, savings: 30 },
];

const APP: ClientApp = {
  id: 'happ',
  name: 'Happ',
  icon: null,
  steps: [
    { kind: 'install', text: { ru: 'Установите приложение из магазина', en: 'Install the app from the store' }, buttons: [{ label: { ru: 'Скачать', en: 'Download' }, url: 'https://example.com' }] },
    { kind: 'add', text: { ru: 'Одна кнопка, ссылка откроется в приложении', en: 'One click opens the link in the app' }, link: 'happ://add/{url}' },
    { kind: 'connect', text: { ru: 'Нажмите кнопку подключения, сервер выберется сам', en: 'Click connect, and the server is picked automatically' } },
  ],
};

export type Demo = { me: ClientMe; answer: Answer };

export function demoOf(site: Site, lang: Lang, state: PreviewState): Demo {
  const names = NAMES[lang];
  const currency = site.currency;
  const rub = currency === 'RUB';
  const money = (minor: number): Money => ({ minor, currency });
  const sum = (usd: number, inRub: number) => money(rub ? inRub : usd);
  const balance = state === 'new' ? money(0) : sum(420, 45_000);

  const period = (terms: Terms): PeriodOffer => ({
    period_id: terms.id,
    days: terms.days,
    months: terms.months,
    price: sum(terms.usd, terms.rub),
    per_month: money(Math.round((rub ? terms.rub : terms.usd) / terms.months)),
    savings_percent: terms.savings,
    stars: terms.stars,
    stars_per_month: Math.round(terms.stars / terms.months),
    stars_savings_percent: terms.savings,
  });
  const offer = (id: number, name: string, text: string, devices: number, terms: Terms[]): TariffOffer => ({
    category: { id: 1, name: names.tariffs, sort: 0, visible: true, subscription_limit: null },
    tariff: {
      id,
      category_id: 1,
      name,
      description: text,
      on_sale: true,
      sort: id,
      device_discounts: [],
      devices: { included: devices, extra: { max: 2, monthly: { usd: 99, rub: 4900 }, monthly_stars: 50 } },
      traffic: { limit_bytes: null, reset: 'no_reset' },
    },
    periods: terms.map(period),
  });
  const catalog: Catalog = { currency, tariffs: [offer(1, names.standard, names.standardText, 3, STANDARD), offer(2, names.max, names.maxText, 5, MAX)] };

  const ended = state === 'ended';
  const left = ended ? 0 : 64;
  const subs: SubscriptionView[] =
    state === 'new'
      ? []
      : [
          {
            subscription: {
              id: 1,
              public_id: 'preview',
              client_id: 1,
              tariff_id: 1,
              kind: 'paid',
              remna: null,
              extra_devices: 0,
              period_started_at: ago((90 - left) * DAY),
              period_days: 90,
              autorenew: { enabled: !ended, period_id: 2 },
              mirror: {
                status: ended ? 'expired' : 'active',
                expire_at: new Date(Date.now() + (ended ? -6 : left) * DAY).toISOString(),
                traffic_used_bytes: 46 * 1024 ** 3,
                traffic_limit_bytes: null,
                hwid_limit: 3,
                devices_count: 2,
              } as SubscriptionView['subscription']['mirror'],
              created_at: ago(400 * DAY),
              ended_at: null,
              device_limit: null,
            },
            status: ended ? 'expired' : 'active',
            progress: { days_left: left, days_total: 90 },
            subscription_url: 'https://sub.example.com/s/9f3c4a7e21',
            countries: ['NL', 'DE', 'FI'],
            tariff: catalog.tariffs[0]?.tariff ?? null,
          },
        ];
  const devices: Device[] = [
    { hwid: 'a', title: 'iPhone 15 Pro', platform: 'iOS', os_version: '18.1', app: 'Happ 3.2', added_at: ago(94 * DAY) },
    { hwid: 'b', title: 'MacBook Air', platform: 'macOS', os_version: '15', app: 'Happ 3.2', added_at: ago(13 * DAY) },
  ];

  const me: ClientMe = {
    client: { id: 1, status: 'active', created_at: ago(400 * DAY), locale: lang },
    locale: lang,
    balance,
    balance_enabled: true,
    promo_codes: true,
    options: true,
    referral_balance: money(0),
    currency,
    channel: 'cabinet',
    full_access: true,
    two_factor: { enabled: false, enabled_at: null },
    payments_count: state === 'new' ? 0 : 7,
    email: 'client@example.com',
    email_change: true,
    has_password: true,
    photo: null,
    support_unread: 1,
  };

  const methods = (total: Money): CheckoutMethod[] => [
    { provider: 'demo-card', method: null, kind: 'external', title: names.card, hint: names.cardHint, amount: total, unavailable: null },
    { provider: 'demo-crypto', method: null, kind: 'external', title: names.crypto, hint: names.cryptoHint, amount: total, unavailable: null },
    {
      provider: 'balance',
      method: null,
      kind: 'balance',
      title: names.balance,
      hint: names.balanceHint,
      amount: total,
      unavailable: balance.minor >= total.minor ? null : { type: 'insufficient_balance', balance, missing: money(total.minor - balance.minor) },
    },
  ];
  const priced = (lines: QuoteLineView[], days: number): Quote['quote'] => {
    const total = money(lines.reduce((all, line) => all + line.amount.minor, 0));
    const stars = lines.reduce((all, line) => all + (line.stars ?? 0), 0);
    return { currency, lines, subtotal: total, discount: money(0), total, stars: stars ? { subtotal: stars, discount: 0, total: stars } : null, days, extra_devices: 0 };
  };
  const quote = (body: { tariff_id?: number; period_id?: number; extra_devices?: number }): Quote => {
    const chosen = catalog.tariffs.find((o) => o.tariff.id === body.tariff_id) ?? (catalog.tariffs[0] as TariffOffer);
    const term = chosen.periods.find((p) => p.period_id === body.period_id) ?? (chosen.periods[1] as PeriodOffer);
    const extra = body.extra_devices ?? 0;
    const lines: QuoteLineView[] = [{ kind: { type: 'period' }, label: `${chosen.tariff.name} · ${term.days}`, amount: term.price, stars: term.stars }];
    if (extra > 0) {
      lines.push({ kind: { type: 'extra_devices' }, label: `+${extra}`, amount: money((rub ? 4900 : 99) * extra * term.months), stars: 50 * extra * term.months });
    }
    const total = priced(lines, term.days);
    const until = new Date(Date.now() + (left + term.days) * DAY).toISOString();
    return {
      quote: { ...total, extra_devices: extra },
      kind: subs.length ? 'renewal' : 'new',
      expire_preview: until,
      methods: methods(total.total),
      current: subs[0] ? { subscription_id: 1, tariff: names.standard, trial: false, expire_at: subs[0].subscription.mirror.expire_at } : null,
    };
  };
  const itemQuote = (body: { module?: string; payload?: { count?: number; period_id?: number } }): ItemQuote => {
    const terms = [...STANDARD, ...MAX].find((term) => term.id === body.payload?.period_id) ?? (STANDARD[1] as Terms);
    const gift = body.module === 'gifts';
    const amount = gift ? sum(terms.usd, terms.rub) : money((rub ? 9800 : 198) * (body.payload?.count ?? 1));
    const total = priced([{ kind: { type: 'module' }, label: gift ? names.standard : `+${body.payload?.count ?? 1}`, amount, stars: null }], terms.days);
    return { quote: total, methods: methods(total.total) };
  };
  const addons: AddonsOffer = {
    subscription_id: 1,
    devices: {
      offer: {
        subscription_id: 1,
        included: 3,
        extra_now: 0,
        can_add: 2,
        days: left,
        monthly: sum(99, 4900),
        monthly_stars: 50,
        per_device: sum(198, 9800),
        per_device_stars: 100,
        expire_at: subs[0]?.subscription.mirror.expire_at ?? ago(0),
        discounts: [],
      },
      unavailable: null,
    },
    traffic: { offer: null, unavailable: { code: 'options.traffic_unlimited', message: '' } },
  };

  const payment = (id: number, days: number, method: string, kind: 'renewal' | 'top_up', status: ClientPayment['status'] = 'paid'): ClientPayment => ({
    id: `preview-${id}`,
    status,
    amount: kind === 'top_up' ? sum(500, 50_000) : sum(699, 49_900),
    refunded: null,
    method,
    autorenew: false,
    repeat: null,
    purchase: {
      kind,
      tariff: kind === 'top_up' ? null : names.standard,
      days: kind === 'top_up' ? null : 90,
      months: kind === 'top_up' ? null : 3,
      label: kind === 'top_up' ? null : names.standard,
      extra_devices: 0,
    },
    created_at: ago(days * DAY),
    paid_at: status === 'paid' ? ago(days * DAY) : null,
    expires_at: null,
    pay_url: null,
  });
  const payments = state === 'new' ? [] : [payment(1, 26, names.card, 'renewal'), payment(2, 40, names.crypto, 'top_up'), payment(3, 116, names.card, 'renewal'), payment(4, 118, names.card, 'renewal', 'canceled')];

  const topup: TopUpOptions = {
    currency,
    balance,
    min: sum(100, 10_000),
    max: sum(100_000, 5_000_000),
    presets: rub ? [50_000, 100_000, 200_000, 500_000].map(money) : [500, 1000, 2000, 5000].map(money),
    methods: [
      { provider: 'demo-card', method: null, title: names.card, hint: names.cardHint },
      { provider: 'demo-crypto', method: null, title: names.crypto, hint: names.cryptoHint, bonus: { percent: 5, min: sum(1000, 100_000) } },
    ],
  };
  const referral: Referral = {
    code: 'DEMO2026',
    bot_link: site.bot_username ? `https://t.me/${site.bot_username.replace(/^@/, '')}?start=demo` : null,
    invited: 4,
    earned: sum(840, 64_000),
    balance: sum(420, 32_000),
    levels: [30, 10],
    withdraw_min: null,
    withdrawals_enabled: true,
    payout_methods: [{ id: 'card', title: names.card, hint: '', min: null }],
    withdrawals: [],
  };
  const gifts: GiftOffers = {
    offers: catalog.tariffs.map((item) => ({ tariff: { id: item.tariff.id, category_id: 1, name: item.tariff.name, description: item.tariff.description }, periods: item.periods })),
    code_days: 90,
  };
  const mine: MyGift[] =
    state === 'new'
      ? []
      : [
          {
            id: 1,
            code: 'GIFT-DEMO-2026',
            tariff_name: names.standard,
            days: 90,
            status: 'active',
            expires_at: new Date(Date.now() + 80 * DAY).toISOString(),
            redeemed_at: null,
            created_at: ago(10 * DAY),
            term: '',
            links: { bot: null, cabinet: null },
          },
        ];
  const ticket = { id: 1, status: 'open' as const, subject: names.subject, last_author: 'support' as const, created_at: ago(5 * HOUR), updated_at: ago(2 * HOUR), closed_at: null, unread: 1, preview: { author: 'support' as const, text: names.reply, at: ago(2 * HOUR) } };
  const thread: ChatThread = {
    ticket,
    messages: [
      { author: 'client', body: names.question, created_at: ago(5 * HOUR) },
      { author: 'support', body: names.reply, created_at: ago(2 * HOUR) },
    ],
    ai: null,
    unread: 0,
  };
  const trial: TrialOffer = { available: state === 'new' && site.trial?.enabled !== false, variants: [] };
  const apps = Object.fromEntries(ALL_PLATFORMS.map((platform) => [platform, [APP]])) as ClientApps;

  const answers: Array<[RegExp, (body: unknown, path: string) => unknown]> = [
    [/^\/api\/client\/me$/, () => me],
    [/^\/api\/client\/subscriptions$/, () => subs],
    [/^\/api\/client\/subscriptions\/\d+$/, () => subs[0] ?? null],
    [/^\/api\/client\/subscriptions\/\d+\/devices$/, () => devices],
    [/^\/api\/client\/catalog$/, () => catalog],
    [/^\/api\/client\/quote$/, (body) => quote((body ?? {}) as Parameters<typeof quote>[0])],
    [/^\/api\/client\/items\/quote$/, (body) => itemQuote((body ?? {}) as Parameters<typeof itemQuote>[0])],
    [/^\/api\/client\/m\/options\/offer\/\d+$/, () => addons],
    [/^\/api\/client\/payments(\?.*)?$/, () => ({ items: payments, next: null })],
    [/^\/api\/client\/topup$/, () => topup],
    [/^\/api\/client\/m\/referral\/me$/, () => referral],
    [/^\/api\/client\/m\/gifts\/offers$/, () => gifts],
    [/^\/api\/client\/m\/gifts\/mine$/, () => mine],
    [/^\/api\/client\/support\/unread$/, () => ({ tickets: 1 })],
    [/^\/api\/client\/support\/tickets$/, () => (state === 'new' ? [] : [ticket])],
    [/^\/api\/client\/support\/tickets\/\d+$/, () => thread],
    [/^\/api\/client\/trial$/, () => trial],
    [/^\/api\/client\/security$/, () => ({ enabled: false, enabled_at: null })],
    [
      /^\/api\/client\/security\/email$/,
      () => ({ email: 'client@example.com', available: true, without_code: false, unconfirmed: false, proof: 'password', confirm_current: false, pending: null }),
    ],
    [
      /^\/api\/client\/security\/logins$/,
      () => ({
        methods: [
          { kind: 'telegram', account: '@client', linked_at: ago(400 * DAY), removable: false },
          { kind: 'email', account: 'client@example.com', linked_at: ago(400 * DAY), removable: false },
          { kind: 'google', account: 'client@example.com', linked_at: ago(90 * DAY), removable: true },
        ],
        can_link: [],
      }),
    ],
    [/^\/api\/public\/connect-apps$/, () => ({ version: 'preview', apps })],
    [/^\/api\/public\/m\/utm\/visit$/, () => ({})],
  ];

  return {
    me,
    answer: (path, options) => {
      const found = answers.find(([pattern]) => pattern.test(path));
      return found ? Promise.resolve(found[1](options.body, path)) : Promise.reject(new ApiError(404, 'preview', ''));
    },
  };
}
