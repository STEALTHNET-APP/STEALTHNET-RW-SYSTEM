import { Fragment, useCallback, useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { ago, money, moneyExact, plural, type Money } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import { ConfirmDialog, parseMinor } from '../client/dialogs';
import { changedConfig, initialValues, missingRequired, SchemaFields, type FieldValues, type SecretState, type SettingField } from '../SchemaFields';
import { useSession } from '../session';
import { Switch } from '../ui';
import '../form.css';
import { SettingsLayout } from './SettingsLayout';
import { TributeSetup, tributeValue, type TributeValue } from './TributeSetup';

type Channels = { cabinet: boolean; bot: boolean; miniapp: boolean };

type MethodView = { code: string; title: string; hint: string; min_minor: number | null; enabled: boolean; custom_title: string | null };

type MethodRow = { code: string; title: string; hint: string; min_minor: number | null; on: boolean; custom: string };

function methodRows(views: MethodView[] | undefined): MethodRow[] {
  return (views ?? []).map((m) => ({ code: m.code, title: m.title, hint: m.hint, min_minor: m.min_minor, on: m.enabled, custom: m.custom_title ?? '' }));
}

function chosenMethods(rows: MethodRow[]): Array<{ code: string; title: string | null }> {
  return rows.filter((r) => r.on).map((r) => ({ code: r.code, title: r.custom.trim() || null }));
}

function settle(rows: MethodRow[]): MethodRow[] {
  return [...rows.filter((r) => r.on), ...rows.filter((r) => !r.on)];
}

type CheckStatus = 'ok' | 'keys_rejected' | 'settings_rejected' | 'unreachable' | 'manual';

type HookCheck = { label: string; state: 'same' | 'other' | 'off'; address: string | null; note: string | null };

type InboxTrace = { verified_at: string | null; rejected_at: string | null };

type CheckRecord = { at: string; status: CheckStatus; summary: string; details: string[]; hooks: HookCheck[]; inbox?: InboxTrace | null };

type TopupBonus = { enabled: boolean; percent: number; min?: Money | null; max?: Money | null; methods?: string[] };

type BonusForm = { on: boolean; percent: string; min: string; max: string; methods: string[] };

const NO_BONUS: BonusForm = { on: false, percent: '', min: '', max: '', methods: [] };

function limitText(limit: Money | null | undefined, currency: Currency): string {
  if (!limit || limit.currency !== currency) {
    return '';
  }
  return limit.minor % 100 ? (limit.minor / 100).toFixed(2) : String(limit.minor / 100);
}

function bonusForm(bonus: TopupBonus | undefined, currency: Currency): BonusForm {
  return bonus
    ? {
        on: bonus.enabled,
        percent: bonus.percent ? String(bonus.percent) : '',
        min: limitText(bonus.min, currency),
        max: limitText(bonus.max, currency),
        methods: bonus.methods ?? [],
      }
    : NO_BONUS;
}

function foreignLimits(bonus: TopupBonus | undefined, currency: Currency): boolean {
  return [bonus?.min, bonus?.max].some((limit) => limit && limit.currency !== currency);
}

function bonusPercent(form: BonusForm): number | null {
  const raw = form.percent.trim();
  if (!raw) {
    return form.on ? null : 0;
  }
  const value = /^\d{1,3}$/.test(raw) ? Number(raw) : null;
  return value !== null && value <= 100 && (value > 0 || !form.on) ? value : null;
}

function bonusLimit(raw: string): number | null | 'wrong' {
  if (!raw.trim()) {
    return null;
  }
  const minor = parseMinor(raw);
  return minor !== null && minor > 0 && minor <= 100_000_000 ? minor : 'wrong';
}

function bonusSummary(p: Provider, currency: Currency): string | null {
  const bonus = p.topup_bonus;
  if (!bonus?.enabled) {
    return null;
  }
  if (foreignLimits(bonus, currency)) {
    return 'Бонус к пополнению не действует: порог или предел задан в другой валюте, сохраните его заново';
  }
  const names = (bonus.methods ?? []).map((code) => {
    const method = p.methods?.find((m) => m.code === code);
    return method?.custom_title || method?.title || code;
  });
  return [
    `Бонус к пополнению: ${bonus.percent}%`,
    bonus.min ? `от ${moneyExact(bonus.min)}` : '',
    bonus.max ? `не больше ${moneyExact(bonus.max)}` : '',
    names.length ? `только ${names.join(', ')}` : '',
  ]
    .filter(Boolean)
    .join(', ');
}

type Provider = {
  id: string;
  title: string;
  kind: string;
  currencies: Currency[];
  enabled: boolean;
  configured: boolean;
  accepts_http_webhooks: boolean;
  supports_recurring: boolean;
  channels?: Channels;
  settings: Record<string, unknown>;
  issue?: { at: string; message: string } | null;
  methods?: MethodView[];
  label?: { title: string; hint: string };
  custom_label?: { title?: string | null; hint?: string | null };
  topup_bonus?: TopupBonus;
  check?: CheckRecord | null;
};

const CHECK_HEADS: Record<CheckStatus, string> = {
  ok: 'Подключение работает',
  keys_rejected: 'Ключи не приняты',
  settings_rejected: 'Ошибка в настройках',
  unreachable: 'Платёжная система не ответила',
  manual: 'Проверить без оплаты нельзя',
};

function hookTrouble(hook: HookCheck): boolean {
  return hook.state !== 'same' || Boolean(hook.note);
}

function checkMark(check: CheckRecord | null | undefined): { text: string; warn: boolean } | null {
  if (!check) {
    return null;
  }
  switch (check.status) {
    case 'ok':
      return check.hooks.some(hookTrouble) || inboxLines(check.inbox, '').some((line) => line.warn)
        ? { text: 'есть замечания', warn: true }
        : { text: `проверено ${ago(check.at)}`, warn: false };
    case 'keys_rejected':
      return { text: 'ключи не приняты', warn: true };
    case 'settings_rejected':
      return { text: 'ошибка в настройках', warn: true };
    case 'unreachable':
      return { text: 'нет ответа при проверке', warn: true };
    case 'manual':
      return null;
  }
}

function hookText(hook: HookCheck): string {
  const note = hook.note ? `. ${hook.note}` : '';
  switch (hook.state) {
    case 'same':
      return hook.note ? `адрес совпадает с адресом панели, но ${hook.note}` : 'адрес совпадает с адресом панели';
    case 'other':
      return `адрес другой${hook.address ? `: ${hook.address}` : ''}${note}`;
    case 'off':
      return `уведомление выключено${note}`;
  }
}

function inboxLines(inbox: InboxTrace | null | undefined, title: string): Array<{ text: string; warn: boolean }> {
  if (!inbox) {
    return [];
  }
  const lines: Array<{ text: string; warn: boolean }> = [];
  const forged = inbox.rejected_at !== null && (inbox.verified_at === null || Date.parse(inbox.rejected_at) > Date.parse(inbox.verified_at));
  if (forged && inbox.rejected_at) {
    lines.push({
      text: `Уведомление ${ago(inbox.rejected_at)} не прошло проверку подписи. Если его прислала ${title}, ключ для уведомлений в панели не совпадает с ключом в кабинете`,
      warn: true,
    });
  }
  if (inbox.verified_at) {
    lines.push({ text: `Последнее уведомление с верной подписью пришло ${ago(inbox.verified_at)}`, warn: false });
  }
  return lines;
}

function CheckResult({ check, title }: { check: CheckRecord; title: string }) {
  const notices = inboxLines(check.inbox, title);
  const trouble = check.hooks.some(hookTrouble) || notices.some((line) => line.warn);
  const tone = check.status === 'ok' ? (trouble ? 'warn' : 'ok') : check.status === 'manual' ? 'plain' : 'bad';
  return (
    <div className={`pay-verify ${tone}`} role="status">
      <span className="pay-verify-head">
        {CHECK_HEADS[check.status]}
        <span className="pay-verify-at">{ago(check.at)}</span>
      </span>
      <span>{check.summary}</span>
      {check.details.map((line) => (
        <span key={line} className="pay-verify-line">
          {line}
        </span>
      ))}
      {notices.map((line) => (
        <span key={line.text} className={line.warn ? 'pay-verify-line trouble' : 'pay-verify-line'}>
          {line.text}
        </span>
      ))}
      {check.hooks.length ? (
        <span className="pay-verify-hooks">
          {check.hooks.map((hook) => (
            <span key={hook.label} className={hookTrouble(hook) ? 'pay-verify-hook trouble' : 'pay-verify-hook'}>
              <span className="pay-verify-label">{hook.label}</span>
              <span>{hookText(hook)}</span>
            </span>
          ))}
        </span>
      ) : null}
    </div>
  );
}

function issueTime(iso: string): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? '' : at.toLocaleString('ru-RU', { day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' });
}

const ALL_CHANNELS: Channels = { cabinet: true, bot: true, miniapp: true };

const CHANNELS: Array<[keyof Channels, string]> = [
  ['bot', 'Бот'],
  ['cabinet', 'Кабинет'],
  ['miniapp', 'Мини-приложение'],
];

type Holders = { clients: number; total: Money };

type Renew = { charge_days_before: number; retry_days: number; enable_for_new: boolean; use_saved_card: boolean };

type Core = { settings: { currency: Currency; public_url: string | null; payment_order: { usd: string[]; rub: string[] } } };

type Work = {
  currency: Currency;
  shown: Record<string, boolean>;
  channels: Record<string, Channels>;
  order: { usd: string[]; rub: string[] };
  charge: string;
  retry: string;
  forNew: boolean;
  savedCard: boolean;
};

const ACCEPTS: Record<string, string> = {
  cryptopay: 'USDT, TON, BTC',
  heleket: 'USDT, BTC, TON и другие монеты',
  cryptomus: 'USDT, BTC, TON и другие монеты',
  lava: 'Карты и СБП в рублях, карты и PayPal в долларах',
  stars: 'Своя цена в звёздах, задаётся в тарифе',
  gram: 'GRAM переводом на кошелёк TON',
  yookassa: 'Карты, СБП, SberPay, T-Pay, автопродление',
  platega: 'СБП, карты, SberPay, криптовалюта',
  yoomoney: 'Карта или кошелёк ЮMoney',
  cloudpayments: 'Карты и СБП',
  overpay: 'Карты и СБП',
  rollypay: 'СБП, карты, криптовалюта',
  mulenpay: 'Карты, СБП, SberPay и T-Pay',
  pal24: 'Карты и СБП',
  kassa_ai: 'СБП, карты, SberPay',
  aurapay: 'Карты и СБП',
  tribute: 'Товары и подписки Tribute: оплата в Telegram',
  wata: 'Карты и СБП',
  freekassa: 'СБП, карты, SberPay и другие способы',
  robokassa: 'Карты, СБП, SberPay, T-Pay',
  cispay: 'СБП, карты, криптовалюта',
  paritypay: 'Карты и СБП',
  antilopay: 'Карты, СБП, SberPay, ЕРИП',
};

const HOOK_KINDS: Record<string, string[]> = { cloudpayments: ['check', 'pay', 'fail', 'refund'], pal24: ['', 'refund', 'chargeback'] };

const OWN_HOOKS: Record<string, string> = {
  heleket: 'Панель передаёт Heleket адрес для уведомлений вместе с каждым счётом, в кабинете Heleket его указывать не нужно.',
  cryptomus: 'Панель передаёт Cryptomus адрес для уведомлений вместе с каждым счётом, в кабинете Cryptomus его указывать не нужно.',
  overpay: 'Панель передаёт Overpay адрес для уведомлений вместе с каждым счётом, в кабинете Overpay его указывать не нужно.',
};

const HOOK_HINTS: Record<string, string> = {
  cloudpayments:
    'В кабинете CloudPayments в настройках сайта включите уведомления Check, Pay, Fail и Refund, каждое на свой адрес из списка выше. Метод GET или POST, кодировка UTF-8, формат CloudPayments.',
  rollypay:
    'У RollyPay это поле callback_url в настройках кассы. Если доступа к кабинету нет, адрес можно передать поддержке RollyPay. Возвраты делаются в кабинете RollyPay, панель отметит их по уведомлению.',
  mulenpay:
    'Адрес указывается в настройках магазина MulenPay. Сама MulenPay уведомления не повторяет, поэтому панель ещё и спрашивает статус счёта. Возврат из панели — только на всю сумму, часть суммы возвращается в кабинете MulenPay.',
  pal24:
    'В настройках магазина PayPalych (Pally): первый адрес — Result URL, второй — Refund URL, третий — Chargeback URL. Success URL и Fail URL — страницы, куда вернётся покупатель, например ваш сайт или бот. Возвраты через API включает поддержка PayPalych.',
  kassa_ai:
    'Это «URL оповещения» в настройках магазина Kassa.ai, метод POST. Уведомления приходят с адресов 168.119.157.136, 168.119.60.227, 178.154.197.79 и 51.250.54.238. Если в магазине включён тестовый режим, тестовые оплаты засчитываются как настоящие: после проверки выключите его.',
  aurapay:
    'Панель передаёт этот адрес с каждым счётом. Если в настройках кассы AuraPay стоит адрес старого бота, замените его этим. Возвратов через API у AuraPay нет: деньги возвращает поддержка AuraPay, а в панели возврат отмечается вручную.',
  tribute:
    'В Tribute это поле Webhook URL: кабинет автора → ⋯ → «Настройки» → «API-ключи». Оплату панель находит по Telegram ID покупателя, поэтому Tribute видят только клиенты с привязанным Telegram.',
  wata:
    'У WATA адрес указывается в кабинете в настройках терминала, у каждого терминала свой. Оплату панель дополнительно сверяет запросом к WATA. Если ссылки не создаются, передайте менеджеру WATA IP-адрес сервера панели.',
  freekassa:
    'У Freekassa это «URL оповещения» в настройках магазина, метод POST. Там же укажите адрес возврата клиента после оплаты, например кабинет. Тестовый режим тоже включается в настройках магазина.',
  robokassa:
    'У Robokassa это Result URL в технических настройках магазина, метод POST. В Success URL и Fail URL укажите, куда вернуть клиента после оплаты, например кабинет.',
  cispay:
    'У CisPay это адрес для вебхуков в «Настройках магазина». Каждое уведомление панель сверяет запросом статуса к CisPay. Платежи из «Песочницы» засчитываются, только если включено «Засчитывать тестовые платежи». Оплату криптовалютой CisPay не возвращает.',
  paritypay:
    'Панель передаёт этот адрес с каждым счётом. Если в настройках кассы ParityPay стоит адрес старого бота, замените его этим. Возвратов через API у ParityPay нет: деньги возвращает её поддержка, а в панели возврат отмечается вручную.',
  antilopay:
    'У Antilopay это адрес для Callback в настройках проекта. Каждое уведомление панель сверяет запросом статуса к Antilopay. Уведомления приходят с адресов 81.177.221.226 и 87.228.9.243. Если Antilopay не пускает к API по IP, разрешите в ней адрес сервера панели.',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function key(currency: Currency): 'usd' | 'rub' {
  return currency === 'RUB' ? 'rub' : 'usd';
}

function fullOrder(saved: string[], providers: Provider[], currency: Currency): string[] {
  const ids = providers.filter((p) => p.kind !== 'balance' && p.currencies.includes(currency)).map((p) => p.id);
  return [...saved.filter((id) => ids.includes(id)), ...ids.filter((id) => !saved.includes(id))];
}

function daysWord(raw: string): string {
  const n = Number.parseInt(raw, 10);
  return plural(Number.isNaN(n) ? 0 : n, 'день', 'дня', 'дней');
}

function same(a: Work, b: Work): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function SettingsPayments() {
  const { reload } = useSession();
  const [providers, setProviders] = useState<Provider[] | null>(null);
  const [holders, setHolders] = useState<Holders | null>(null);
  const [publicUrl, setPublicUrl] = useState<string | null>(null);
  const [base, setBase] = useState<Work | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [setup, setSetup] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [pay, core, renew] = await Promise.all([
        api<{ currency: Currency; order: string[]; providers: Provider[]; balances?: Holders }>('/api/admin/settings/payments'),
        api<Core>('/api/admin/settings/core'),
        api<Renew>('/api/admin/settings/autorenew'),
      ]);
      const list = pay.providers;
      const next: Work = {
        currency: pay.currency,
        shown: Object.fromEntries(list.map((p) => [p.id, p.enabled && p.configured])),
        channels: Object.fromEntries(list.map((p) => [p.id, p.channels ?? ALL_CHANNELS])),
        order: { usd: fullOrder(core.settings.payment_order.usd, list, 'USD'), rub: fullOrder(core.settings.payment_order.rub, list, 'RUB') },
        charge: String(renew.charge_days_before),
        retry: String(renew.retry_days),
        forNew: renew.enable_for_new,
        savedCard: renew.use_saved_card,
      };
      setProviders(list);
      setHolders(pay.balances ?? null);
      setPublicUrl(core.settings.public_url);
      setBase(next);
      setWork(next);
      setLoadError(null);
    } catch (err) {
      setLoadError(failText(err, 'Настройки оплаты не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const dirty = base !== null && work !== null && !same(base, work);

  const edit = (patch: Partial<Work>) => {
    setWork((current) => (current ? { ...current, ...patch } : current));
    setStatus(null);
  };

  const visible = useMemo(() => (providers && work ? providers.filter((p) => p.kind !== 'balance' && p.currencies.includes(work.currency)) : []), [providers, work]);

  if (loadError && !work) {
    return (
      <SettingsLayout>
        <p className="err" role="alert">
          {loadError}
        </p>
      </SettingsLayout>
    );
  }

  if (!providers || !work || !base) {
    return (
      <SettingsLayout>
        <p className="muted">Загружаем настройки оплаты…</p>
      </SettingsLayout>
    );
  }

  const cur = key(work.currency);
  const shownOrder = work.order[cur].filter((id) => work.shown[id]);
  const titleOf = (id: string) => providers.find((p) => p.id === id)?.title ?? id;
  const yookassaOn = work.shown.yookassa === true;

  const move = (id: string, step: -1 | 1) => {
    const i = shownOrder.indexOf(id);
    const other = shownOrder[i + step];
    if (other === undefined) {
      return;
    }
    const full = [...work.order[cur]];
    const a = full.indexOf(id);
    const b = full.indexOf(other);
    full[a] = other;
    full[b] = id;
    edit({ order: { ...work.order, [cur]: full } });
  };

  const digits = (raw: string) => raw.replace(/\D/g, '').slice(0, 2);
  const numberOk = (raw: string) => raw !== '' && Number(raw) <= 30;

  const save = async (confirmed = false) => {
    if (!dirty || busy) {
      return;
    }
    if (!numberOk(work.charge) || !numberOk(work.retry)) {
      setStatus({ text: 'Дни автопродления: число от 0 до 30', bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      const savedKey = key(base.currency);
      if (work.currency !== base.currency) {
        if (JSON.stringify(work.order[savedKey]) !== JSON.stringify(base.order[savedKey])) {
          await api('/api/admin/settings/payments', { method: 'PUT', body: { order: work.order[savedKey] } });
        }
        try {
          await api('/api/admin/settings/core', { method: 'PUT', body: { currency: work.currency, confirm_currency_change: confirmed } });
        } catch (err) {
          if (err instanceof ApiError && err.code === 'settings.currency_balances') {
            setConfirm(err.message);
            setBusy(false);
            return;
          }
          throw err;
        }
      }
      if (JSON.stringify(work.order[cur]) !== JSON.stringify(base.order[cur]) || work.currency !== base.currency) {
        await api('/api/admin/settings/payments', { method: 'PUT', body: { order: work.order[cur] } });
      }
      for (const p of providers) {
        const body: Record<string, unknown> = {};
        if (p.configured && work.shown[p.id] !== base.shown[p.id]) {
          body.enabled = work.shown[p.id];
        }
        if (JSON.stringify(work.channels[p.id]) !== JSON.stringify(base.channels[p.id])) {
          body.channels = work.channels[p.id];
        }
        if (Object.keys(body).length > 0) {
          await api(`/api/admin/payments/providers/${p.id}`, { method: 'PUT', body });
        }
      }
      if (work.charge !== base.charge || work.retry !== base.retry || work.forNew !== base.forNew || work.savedCard !== base.savedCard) {
        await api('/api/admin/settings/autorenew', {
          method: 'PUT',
          body: { charge_days_before: Number(work.charge), retry_days: Number(work.retry), enable_for_new: work.forNew, use_saved_card: work.savedCard },
        });
      }
      const currencyChanged = work.currency !== base.currency;
      await load();
      if (currencyChanged) {
        await reload();
      }
      setStatus({ text: 'Сохранено', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки не сохранились'), bad: true });
      await load().catch(() => undefined);
    } finally {
      setBusy(false);
    }
  };

  const configured = (next: Provider) => {
    setProviders(providers.map((p) => (p.id === next.id ? next : p)));
    const shown = next.enabled && next.configured;
    setBase({ ...base, shown: { ...base.shown, [next.id]: shown }, channels: { ...base.channels, [next.id]: next.channels ?? ALL_CHANNELS } });
    setWork({ ...work, shown: { ...work.shown, [next.id]: shown } });
  };

  const checked = (id: string, check: CheckRecord) => {
    setProviders((list) => (list ? list.map((p) => (p.id === id ? { ...p, check } : p)) : list));
  };

  const toggleChannel = (id: string, channel: keyof Channels) => {
    const current = work.channels[id] ?? ALL_CHANNELS;
    edit({ channels: { ...work.channels, [id]: { ...current, [channel]: !current[channel] } } });
  };

  const channelLine = (id: string, title: string) => {
    const current = work.channels[id] ?? ALL_CHANNELS;
    const none = !current.bot && !current.cabinet && !current.miniapp;
    return (
      <div className="pay-channels" role="group" aria-label={`Где показывать: ${title}`}>
        <span className="pay-ch-title muted">Где показывать</span>
        {CHANNELS.map(([channel, label]) => (
          <label key={channel} className="pay-check">
            <input type="checkbox" checked={current[channel]} onChange={() => toggleChannel(id, channel)} disabled={busy} />
            {label}
          </label>
        ))}
        {none ? <span className="warn-line">Нигде не показывается</span> : null}
      </div>
    );
  };

  const methodLine = (p: Provider) => {
    const on = (p.methods ?? []).filter((m) => m.enabled);
    if (on.length) {
      return <p className="pay-mline muted">Кнопки у клиента: {on.map((m) => m.custom_title || m.title).join(' · ')}</p>;
    }
    const custom = p.custom_label?.title;
    return custom ? <p className="pay-mline muted">Кнопка у клиента: «{custom}»</p> : null;
  };

  const balance = providers.find((p) => p.kind === 'balance');
  const balanceOn = balance ? work.shown[balance.id] === true : true;

  const statusText = status?.text ?? (dirty ? 'Есть несохранённые изменения' : '');

  return (
    <SettingsLayout>
      <section className="sec pay-currency" aria-labelledby="cur-h">
        <div className="pay-currency-text">
          <h2 id="cur-h" className="section-title">
            Валюта сервиса
          </h2>
          <span className="muted">Цены, баланс клиентов и платёжные системы работают в одной валюте, без курсов и конвертации</span>
        </div>
        <div role="group" aria-labelledby="cur-h" className="segs">
          {(
            [
              ['USD', 'Доллары, $'],
              ['RUB', 'Рубли, ₽'],
            ] as const
          ).map(([code, label]) => (
            <button key={code} type="button" aria-pressed={work.currency === code} onClick={() => edit({ currency: code })} disabled={busy}>
              {label}
            </button>
          ))}
        </div>
      </section>

      <section className="sec" aria-labelledby="ps-h">
        <div className="sec-head">
          <h2 id="ps-h" className="section-title">
            Платёжные системы
          </h2>
          <span className="muted">Для валюты «{work.currency === 'RUB' ? 'Рубли' : 'Доллары'}»</span>
          <span className="pay-rule">
            Правила Telegram: в боте и мини-приложении цифровые товары, включая VPN-подписку, продаются только за звёзды. За другие способы оплаты там Telegram может заблокировать бота без
            предупреждения, поэтому карты и криптовалюту безопаснее оставить только в кабинете.
          </span>
        </div>
        {visible.map((p) => {
          const on = work.shown[p.id] === true;
          const mark = checkMark(p.check);
          return (
            <Fragment key={p.id}>
              <div className="pay-row">
                <span className="pay-name">{p.title}</span>
                <span className="pay-what muted">{ACCEPTS[p.id] ?? ''}</span>
                <span className="pay-state" style={{ color: p.configured ? 'var(--ink-2)' : 'var(--warn)' }}>
                  {p.configured ? 'Подключена' : 'Не настроена'}
                  {p.configured && mark ? <span className={mark.warn ? 'pay-checked warn' : 'pay-checked'}>{mark.text}</span> : null}
                </span>
                <span className={p.configured ? 'pay-switch' : 'pay-switch off'}>
                  <Switch on={on} onToggle={() => p.configured && edit({ shown: { ...work.shown, [p.id]: !on } })} label={`Показывать клиентам: ${p.title}`} disabled={!p.configured || busy} />
                </span>
                <button type="button" className="pay-setup hov" aria-expanded={setup === p.id} aria-label={`Настроить: ${p.title}`} onClick={() => setSetup(setup === p.id ? null : p.id)}>
                  Настроить
                </button>
              </div>
              {channelLine(p.id, p.title)}
              {methodLine(p)}
              {bonusSummary(p, base.currency) ? <p className="pay-mline muted">{bonusSummary(p, base.currency)}</p> : null}
              {p.issue ? (
                <p className="pay-issue" role="status">
                  <span className="pay-issue-at">Проверка оплат, {issueTime(p.issue.at)}</span>
                  {p.issue.message}
                </p>
              ) : null}
              {setup === p.id ? (
                <ProviderSetup provider={p} currency={base.currency} publicUrl={publicUrl} onClose={() => setSetup(null)} onSaved={configured} onChecked={(check) => checked(p.id, check)} />
              ) : null}
            </Fragment>
          );
        })}
        {balance ? (
          <>
            <div className="pay-row">
              <span className="pay-name">С баланса</span>
              <span className="pay-what muted">Клиент пополняет баланс и платит с него</span>
              <span className="pay-state" style={{ color: 'var(--ink-2)' }}>
                {balanceOn ? 'Включена' : 'Выключена'}
              </span>
              <span className="pay-switch">
                <Switch on={balanceOn} onToggle={() => edit({ shown: { ...work.shown, [balance.id]: !balanceOn } })} label="Пополнение и оплата с баланса" disabled={busy} />
              </span>
              <button type="button" className="pay-setup hov" aria-expanded={setup === balance.id} aria-label="Настроить: С баланса" onClick={() => setSetup(setup === balance.id ? null : balance.id)}>
                Настроить
              </button>
            </div>
            {setup === balance.id ? (
              <ProviderSetup provider={balance} currency={base.currency} publicUrl={publicUrl} onClose={() => setSetup(null)} onSaved={configured} onChecked={(check) => checked(balance.id, check)} />
            ) : null}
            {methodLine(balance)}
            {balanceOn ? (
              channelLine(balance.id, 'С баланса')
            ) : (
              <div className="pay-off">
                <span>Пополнить баланс нельзя ни в боте, ни в кабинете, ни в мини-приложении: клиент платит сразу через платёжную систему.</span>
                <span>
                  Деньги, которые уже лежат на балансе, не пропадают: ими можно оплатить подписку или автопродление, если остатка хватает на всю сумму. Остаток меньше цены можно вернуть клиенту вручную и
                  списать в его карточке.
                </span>
                {holders ? (
                  <span className="pay-off-sum">
                    {holders.clients > 0
                      ? `Сейчас деньги на балансе у ${holders.clients} ${plural(holders.clients, 'клиента', 'клиентов', 'клиентов')}, всего ${money(holders.total)}`
                      : 'Сейчас ни у кого из клиентов нет денег на балансе'}
                  </span>
                ) : null}
              </div>
            )}
          </>
        ) : null}
      </section>

      <div className="pay-pair">
        <section className="sec pay-card" aria-labelledby="ord-h">
          <div className="sec-head">
            <h2 id="ord-h" className="section-title">
              Порядок у клиента
            </h2>
            <span className="muted">
              Одинаковый в боте, кабинете и <span style={{ whiteSpace: 'nowrap' }}>мини-приложении</span>
            </span>
          </div>
          {shownOrder.length === 0 ? (
            <p className="pay-empty warn-line">Включите хотя бы одну платёжную систему</p>
          ) : (
            <ol className="pay-order">
              {shownOrder.map((id, i) => (
                <li key={id}>
                  <span className="num pay-pos">{i + 1}</span>
                  <span className="pay-oname">{titleOf(id)}</span>
                  <button type="button" className="pay-arrow" aria-label={`Выше: ${titleOf(id)}`} aria-disabled={i === 0} onClick={() => i > 0 && move(id, -1)} disabled={busy}>
                    <Icon name="arrow-up" size={18} />
                  </button>
                  <button
                    type="button"
                    className="pay-arrow"
                    aria-label={`Ниже: ${titleOf(id)}`}
                    aria-disabled={i === shownOrder.length - 1}
                    onClick={() => i < shownOrder.length - 1 && move(id, 1)}
                    disabled={busy}
                  >
                    <Icon name="arrow-down" size={18} />
                  </button>
                </li>
              ))}
            </ol>
          )}
          {balanceOn ? <p className="pay-foot cap">Оплата с баланса всегда последняя</p> : null}
        </section>

        <section className="sec pay-card" aria-labelledby="ren-h">
          <div className="sec-head">
            <h2 id="ren-h" className="section-title">
              Автопродление
            </h2>
            <span className="muted">{balanceOn ? 'Списывается с баланса клиента' : 'Баланс выключен: спишем остаток, если его хватает, иначе напомним продлить вручную'}</span>
          </div>
          <div className="pay-renew">
            <label className="pay-line">
              <span className="pay-lbl">Списывать за</span>
              <input className="fld num" inputMode="numeric" value={work.charge} onChange={(e) => edit({ charge: digits(e.target.value) })} aria-label="За сколько дней до окончания списывать" disabled={busy} />
              <span className="pay-lbl">{daysWord(work.charge)} до окончания</span>
            </label>
          </div>
          <div className="pay-renew">
            <label className="pay-line">
              <span className="pay-lbl">Повторять попытки</span>
              <input className="fld num" inputMode="numeric" value={work.retry} onChange={(e) => edit({ retry: digits(e.target.value) })} aria-label="Сколько дней повторять попытки" disabled={busy} />
              <span className="pay-lbl">{daysWord(work.retry)}</span>
            </label>
            <span className="cap">Потом автопродление выключится</span>
          </div>
          <div className="pay-renew pay-toggle">
            <span className="sf-text">
              <span className="pay-lbl">Включать новым клиентам</span>
              <span className="cap">Клиент может выключить его в кабинете</span>
            </span>
            <Switch on={work.forNew} onToggle={() => edit({ forNew: !work.forNew })} label="Включать автопродление новым клиентам" disabled={busy} />
          </div>
          {work.currency === 'RUB' ? (
            <div className="pay-renew pay-toggle">
              <span className="sf-text">
                <span className="pay-lbl">Списывать с сохранённой карты через ЮKassa</span>
                {yookassaOn ? (
                  <span className="cap">Если на балансе не хватает. Карты сохраняются, только когда в настройках ЮKassa включены «Автоплатежи»</span>
                ) : (
                  <span className="warn-line">Работает, когда ЮKassa включена</span>
                )}
              </span>
              <span className={yookassaOn ? 'pay-switch' : 'pay-switch off'}>
                <Switch on={yookassaOn && work.savedCard} onToggle={() => yookassaOn && edit({ savedCard: !work.savedCard })} label="Списывать с сохранённой карты через ЮKassa" disabled={!yookassaOn || busy} />
              </span>
            </div>
          ) : null}
        </section>
      </div>

      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button
          type="button"
          className="btn-line hov"
          disabled={busy}
          onClick={() => {
            setWork(base);
            setStatus(null);
          }}
        >
          Отменить
        </button>
        <span role="status" className={status?.bad ? 'bad' : undefined}>
          {statusText}
          {status?.bad && status.text.startsWith('Сначала задайте цены') ? (
            <>
              {' '}
              <Link to="/admin/tariffs" className="hm-link">
                Открыть тарифы
              </Link>
            </>
          ) : null}
        </span>
      </div>

      {confirm ? (
        <ConfirmDialog
          title="Сменить валюту?"
          text={confirm}
          confirm="Сменить валюту"
          run={async () => {
            setConfirm(null);
            await save(true);
          }}
          onClose={() => setConfirm(null)}
        />
      ) : null}
    </SettingsLayout>
  );
}

type SetupProps = {
  provider: Provider;
  currency: Currency;
  publicUrl: string | null;
  onClose: () => void;
  onSaved: (provider: Provider) => void;
  onChecked: (check: CheckRecord) => void;
};

function ProviderSetup({ provider, currency, publicUrl, onClose, onSaved, onChecked }: SetupProps) {
  const [schema, setSchema] = useState<SettingField[] | null>(null);
  const [config, setConfig] = useState<Record<string, unknown>>({});
  const [values, setValues] = useState<FieldValues>({});
  const [secrets, setSecrets] = useState<SecretState>({ clear: [], replacing: [] });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [rows, setRows] = useState<MethodRow[]>([]);
  const [savedRows, setSavedRows] = useState('[]');
  const [labelTitle, setLabelTitle] = useState('');
  const [labelHint, setLabelHint] = useState('');
  const [savedLabel, setSavedLabel] = useState('');
  const [connected, setConnected] = useState(provider.configured);
  const [check, setCheck] = useState<CheckRecord | null>(provider.check ?? null);
  const [checking, setChecking] = useState(false);
  const [saved, setSaved] = useState(false);
  const [bonus, setBonus] = useState<BonusForm>(NO_BONUS);
  const [savedBonus, setSavedBonus] = useState(JSON.stringify(NO_BONUS));
  const [foreign, setForeign] = useState(false);
  const [tribute, setTribute] = useState<TributeValue>({ products: [], plans: [] });
  const [savedTribute, setSavedTribute] = useState(JSON.stringify({ products: [], plans: [] }));
  const withTribute = provider.id === 'tribute';
  const withBonus = provider.kind === 'external';
  const percent = bonusPercent(bonus);
  const minLimit = bonusLimit(bonus.min);
  const maxLimit = bonusLimit(bonus.max);
  const offered = rows.filter((r) => r.on);
  const pickable = offered.length > 1 || bonus.methods.length > 0;
  const covers = (code: string) => bonus.methods.length === 0 || bonus.methods.includes(code);
  const nowhere = bonus.methods.length > 0 && !offered.some((r) => bonus.methods.includes(r.code));
  const bonusPending = withBonus && JSON.stringify(bonus) !== savedBonus;

  useEffect(() => {
    api<{ provider: Provider; schema: SettingField[] }>(`/api/admin/payments/providers/${provider.id}`)
      .then((d) => {
        setSchema(d.schema);
        setConfig(d.provider.settings);
        setValues(initialValues(d.schema, d.provider.settings));
        const loadedTribute = tributeValue(d.provider.settings);
        setTribute(loadedTribute);
        setSavedTribute(JSON.stringify(loadedTribute));
        const loaded = settle(methodRows(d.provider.methods));
        setRows(loaded);
        setSavedRows(JSON.stringify(chosenMethods(loaded)));
        const title = d.provider.custom_label?.title ?? '';
        const hint = d.provider.custom_label?.hint ?? '';
        setLabelTitle(title);
        setLabelHint(hint);
        setSavedLabel(JSON.stringify([title.trim(), hint.trim()]));
        const loadedBonus = bonusForm(d.provider.topup_bonus, currency);
        setBonus(loadedBonus);
        setSavedBonus(JSON.stringify(loadedBonus));
        setForeign(foreignLimits(d.provider.topup_bonus, currency));
      })
      .catch((err) => setError(failText(err, 'Настройки системы не загрузились')));
  }, [provider.id, currency]);

  const origin = (publicUrl ?? window.location.origin).replace(/\/+$/, '');
  const ownHooks = OWN_HOOKS[provider.id];
  const hooks =
    provider.accepts_http_webhooks && !ownHooks
      ? (HOOK_KINDS[provider.id] ?? ['']).map((kind) => ({ kind, url: `${origin}/api/webhooks/pay/${provider.id}${kind ? `/${kind}` : ''}` }))
      : [];

  const copy = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(url);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      setError('Браузер не дал скопировать: выделите адрес вручную');
    }
  };

  const toggle = (code: string) => setRows((list) => settle(list.map((r) => (r.code === code ? { ...r, on: !r.on } : r))));

  const toggleBonus = (code: string) => {
    const codes = offered.map((r) => r.code);
    const picked = bonus.methods.length ? bonus.methods : codes;
    const next = covers(code) ? picked.filter((c) => c !== code) : [...picked, code];
    if (next.length > 0) {
      const all = next.length === codes.length && codes.every((c) => next.includes(c));
      setBonus({ ...bonus, methods: all ? [] : next });
    }
  };

  const rename = (code: string, custom: string) => setRows((list) => list.map((r) => (r.code === code ? { ...r, custom } : r)));

  const raise = (code: string) =>
    setRows((list) => {
      const i = list.findIndex((r) => r.code === code);
      const above = list[i - 1];
      const row = list[i];
      if (i <= 0 || !above || !row) {
        return list;
      }
      const next = [...list];
      next[i - 1] = row;
      next[i] = above;
      return next;
    });

  const keysPending = schema !== null && (Object.keys(changedConfig(schema, config, values)).length > 0 || secrets.clear.length > 0);
  const tributePending = withTribute && JSON.stringify(tribute) !== savedTribute;
  const pending =
    keysPending ||
    tributePending ||
    bonusPending ||
    JSON.stringify(chosenMethods(rows)) !== savedRows ||
    JSON.stringify([labelTitle.trim(), labelHint.trim()]) !== savedLabel;
  const checkable = provider.kind !== 'balance';

  const runCheck = async () => {
    setChecking(true);
    setError(null);
    try {
      const result = await api<CheckRecord>(`/api/admin/payments/providers/${provider.id}/check`, { method: 'POST' });
      setCheck(result);
      onChecked(result);
    } catch (err) {
      setError(failText(err, 'Проверка не выполнена'));
    } finally {
      setChecking(false);
    }
  };

  const save = async () => {
    if (!schema) {
      return;
    }
    const lacking = missingRequired(schema, config, values, secrets);
    if (lacking.length) {
      setError(`Заполните: ${lacking.map((f) => f.label).join(', ')}`);
      return;
    }
    if (withBonus && bonusPending) {
      if (percent === null) {
        setError('Процент бонуса — целое число от 1 до 100');
        return;
      }
      if (minLimit === 'wrong' || maxLimit === 'wrong') {
        setError('Сумма в настройках бонуса — число больше нуля, пустое поле — без ограничения');
        return;
      }
    }
    setBusy(true);
    setError(null);
    try {
      const keysChanged = keysPending;
      const methods = chosenMethods(rows);
      const label = [labelTitle.trim(), labelHint.trim()];
      const result = await api<{ provider: Provider; schema: SettingField[] }>(`/api/admin/payments/providers/${provider.id}`, {
        method: 'PUT',
        body: {
          config: { ...changedConfig(schema, config, values), ...(tributePending ? { products: tribute.products, plans: tribute.plans } : {}) },
          clear: secrets.clear,
          ...(JSON.stringify(methods) !== savedRows ? { methods } : {}),
          ...(bonusPending
            ? {
                topup_bonus: {
                  enabled: bonus.on,
                  percent,
                  min_minor: minLimit,
                  max_minor: maxLimit,
                  methods: bonus.methods.filter((code) => rows.some((r) => r.code === code)),
                },
              }
            : {}),
          ...(JSON.stringify(label) !== savedLabel ? { label: { title: label[0] || null, hint: label[1] || null } } : {}),
        },
      });
      onSaved(result.provider);
      setSavedTribute(JSON.stringify(tribute));
      if (!keysChanged || !checkable || !result.provider.configured) {
        onClose();
        return;
      }
      setConfig(result.provider.settings);
      setValues(initialValues(result.schema, result.provider.settings));
      setSecrets({ clear: [], replacing: [] });
      setSavedRows(JSON.stringify(methods));
      setSavedLabel(JSON.stringify(label));
      const keptBonus = bonusForm(result.provider.topup_bonus, currency);
      setBonus(keptBonus);
      setSavedBonus(JSON.stringify(keptBonus));
      setForeign(foreignLimits(result.provider.topup_bonus, currency));
      setConnected(true);
      setCheck(null);
      setSaved(true);
      setBusy(false);
      await runCheck();
    } catch (err) {
      setError(failText(err, 'Настройки системы не сохранились'));
      setBusy(false);
    }
  };

  return (
    <div className="pay-setup-panel" role="group" aria-label={`Настройка ${provider.title}`}>
      {schema === null && !error ? <p className="muted">Загружаем…</p> : null}
      {schema && schema.length === 0 && provider.kind === 'stars' ? (
        <p className="muted">Звёзды принимает бот сервиса: отдельных ключей не нужно. Система станет «Подключена», когда в настройках будет указан токен бота.</p>
      ) : null}
      {schema && schema.length > 0 ? (
        <SchemaFields schema={schema} config={config} values={values} onChange={setValues} secrets={secrets} onSecrets={setSecrets} busy={busy} idPrefix={`pay-${provider.id}`} />
      ) : null}
      {schema && withTribute ? <TributeSetup value={tribute} onChange={setTribute} busy={busy} connected={connected} currency={currency} /> : null}
      {rows.length ? (
        <div className="sf-field" style={{ maxWidth: 'none' }} role="group" aria-labelledby={`pay-m-${provider.id}`}>
          <span className="sf-label" id={`pay-m-${provider.id}`}>
            Способы оплаты
          </span>
          <span className="cap">
            Отмеченные способы станут отдельными кнопками у клиента, в этом порядке. Если ничего не отмечено, у клиента одна кнопка «{provider.label?.title ?? provider.title}».
          </span>
          {rows.map((r, i) => (
            <div key={r.code} className="pay-mrow">
              <label className="pay-check pay-mname">
                <input type="checkbox" checked={r.on} onChange={() => toggle(r.code)} disabled={busy} />
                {r.title}
              </label>
              <span className="pay-mhint muted">
                {r.hint}
                {r.min_minor ? ` · от ${money({ minor: r.min_minor, currency })}` : ''}
              </span>
              {r.on ? (
                <span className="pay-mtools">
                  <input
                    className="field pay-mtitle"
                    value={r.custom}
                    placeholder={r.title}
                    maxLength={40}
                    aria-label={`Название кнопки вместо «${r.title}»`}
                    onChange={(e) => rename(r.code, e.target.value)}
                    disabled={busy}
                  />
                  <button type="button" className="pay-arrow" aria-label={`Выше: ${r.title}`} aria-disabled={i === 0} onClick={() => raise(r.code)} disabled={busy}>
                    <Icon name="arrow-up" size={18} />
                  </button>
                </span>
              ) : null}
            </div>
          ))}
        </div>
      ) : null}
      {schema && !rows.some((r) => r.on) ? (
        <div className="sf-field" style={{ maxWidth: 'none' }} role="group" aria-labelledby={`pay-l-${provider.id}`}>
          <span className="sf-label" id={`pay-l-${provider.id}`}>
            Кнопка у клиента
          </span>
          <span className="cap">Так кнопка называется в боте, кабинете и мини-приложении. Пустое поле — текст по умолчанию.</span>
          <span className="pay-lrow">
            <input
              className="field pay-ltitle"
              value={labelTitle}
              placeholder={provider.label?.title || provider.title}
              maxLength={40}
              aria-label="Название кнопки"
              onChange={(e) => setLabelTitle(e.target.value)}
              disabled={busy}
            />
            <input
              className="field pay-lhint"
              value={labelHint}
              placeholder={provider.label?.hint || 'Пояснение под названием'}
              maxLength={80}
              aria-label="Пояснение под названием кнопки"
              onChange={(e) => setLabelHint(e.target.value)}
              disabled={busy}
            />
          </span>
        </div>
      ) : null}
      {schema && withBonus ? (
        <div className="sf-field pay-bonus" role="group" aria-labelledby={`pay-b-${provider.id}`}>
          <span className="pay-bonus-head">
            <span className="sf-label" id={`pay-b-${provider.id}`}>
              Бонус к пополнению
            </span>
            <Switch
              on={bonus.on}
              onToggle={() => setBonus({ ...bonus, on: !bonus.on })}
              label={`Бонус к пополнению через ${provider.title}`}
              describedBy={`pay-bh-${provider.id}`}
              disabled={busy}
            />
          </span>
          <span className="cap" id={`pay-bh-${provider.id}`}>
            Процент от суммы придёт на баланс сверх пополнения этим способом. При возврате бонус снимается.
          </span>
          {foreign ? <span className="warn-line">Порог или предел задан в другой валюте. Бонус не действует, пока настройки не сохранены заново.</span> : null}
          {bonus.on ? (
            <>
              <span className="pay-lrow">
                <label className="pay-bonus-field">
                  <span className="cap">Процент</span>
                  <input
                    className="field num"
                    inputMode="numeric"
                    maxLength={3}
                    placeholder="5"
                    value={bonus.percent}
                    onChange={(e) => setBonus({ ...bonus, percent: e.target.value })}
                    aria-invalid={(bonus.percent.trim() !== '' && percent === null) || undefined}
                    disabled={busy}
                  />
                </label>
                <label className="pay-bonus-field wide">
                  <span className="cap">Действует от суммы пополнения, {currency === 'RUB' ? '₽' : '$'}</span>
                  <input
                    className="field num"
                    inputMode="decimal"
                    maxLength={12}
                    placeholder="Без ограничения"
                    value={bonus.min}
                    onChange={(e) => setBonus({ ...bonus, min: e.target.value })}
                    aria-invalid={minLimit === 'wrong' || undefined}
                    disabled={busy}
                  />
                </label>
                <label className="pay-bonus-field wide">
                  <span className="cap">Бонус не больше, {currency === 'RUB' ? '₽' : '$'}</span>
                  <input
                    className="field num"
                    inputMode="decimal"
                    maxLength={12}
                    placeholder="Без ограничения"
                    value={bonus.max}
                    onChange={(e) => setBonus({ ...bonus, max: e.target.value })}
                    aria-invalid={maxLimit === 'wrong' || undefined}
                    disabled={busy}
                  />
                </label>
              </span>
              {bonus.percent.trim() !== '' && percent === null ? <span className="err">Процент — целое число от 1 до 100</span> : null}
              {minLimit === 'wrong' || maxLimit === 'wrong' ? <span className="err">Сумма — число больше нуля, пустое поле — без ограничения</span> : null}
              {pickable ? (
                <span className="pay-bonus-methods" role="group" aria-label="Для каких способов действует бонус">
                  <span className="cap">Для каких способов</span>
                  {rows
                    .filter((r) => r.on || bonus.methods.includes(r.code))
                    .map((r) => (
                      <label key={r.code} className="pay-check">
                        <input type="checkbox" checked={covers(r.code)} onChange={() => toggleBonus(r.code)} disabled={busy} />
                        {r.custom.trim() || r.title}
                      </label>
                    ))}
                </span>
              ) : null}
              {nowhere ? <span className="warn-line">Выбранные способы не показываются клиентам, бонус никто не получит</span> : null}
            </>
          ) : null}
        </div>
      ) : null}
      {hooks.length ? (
        <div className="sf-field" style={{ maxWidth: 'none' }}>
          <span className="sf-label">{hooks.length > 1 ? 'Адреса для уведомлений' : 'Адрес для уведомлений'}</span>
          {hooks.map((hook) => (
            <span key={hook.url} className="pay-hook">
              {hook.kind ? <span className="muted">{hook.kind}</span> : null}
              <code className="mono">{hook.url}</code>
              <button type="button" className="b34 hov" onClick={() => void copy(hook.url)}>
                {copied === hook.url ? 'Скопировано' : 'Скопировать'}
              </button>
            </span>
          ))}
          <span className="cap">Укажите в кабинете {provider.title}, чтобы оплаты засчитывались сразу</span>
          {HOOK_HINTS[provider.id] ? <span className="cap">{HOOK_HINTS[provider.id]}</span> : null}
        </div>
      ) : null}
      {ownHooks ? (
        <div className="sf-field" style={{ maxWidth: 'none' }}>
          <span className="sf-label">Уведомления</span>
          <span className="cap">{ownHooks}</span>
        </div>
      ) : null}
      {checkable && schema ? (
        <div className="sf-field" style={{ maxWidth: 'none' }}>
          <span className="sf-label">Проверка подключения</span>
          {saved && checking ? <span className="cap">Ключи сохранены, проверяем их запросом в {provider.title}</span> : null}
          {check ? (
            <CheckResult check={check} title={provider.title} />
          ) : (
            <span className="cap">
              {connected
                ? `Панель отправит в ${provider.title} запрос с сохранёнными ключами. Он ничего не создаёт и не списывает деньги.`
                : 'Проверить можно после того, как ключи будут заполнены и сохранены.'}
            </span>
          )}
          {keysPending && connected ? <span className="cap">Проверяются сохранённые ключи: сначала сохраните изменения.</span> : null}
        </div>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="cc-btns" style={{ gap: 10 }}>
        {schema ? (
          <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || checking}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
        ) : null}
        {checkable && schema ? (
          <button type="button" className="btn-line hov" onClick={() => void runCheck()} disabled={busy || checking || !connected || keysPending} style={{ padding: '0 16px' }}>
            {checking ? 'Проверяем…' : 'Проверить подключение'}
          </button>
        ) : null}
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy} style={{ padding: '0 16px' }}>
          {pending ? 'Отменить' : 'Закрыть'}
        </button>
      </div>
    </div>
  );
}
