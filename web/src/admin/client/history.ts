import { count, dateLong, days, money, moneyExact, periodLabel, timeHM, type Money } from '../../lib/format';
import { START_VIA_TEXT, startMark, startVia } from '../../lib/utmTags';
import type { Amount, HistoryItem, OrderBrief, Person } from './types';

export type HistoryLine = { key: string; when: string; what: string; note: string; sum: string; refund?: number };

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

export function historyWhen(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  const diff = Math.round((startOfDay(new Date()) - startOfDay(date)) / 86_400_000);
  if (diff === 0) {
    return `Сегодня, ${timeHM(iso)}`;
  }
  if (diff === 1) {
    return `Вчера, ${timeHM(iso)}`;
  }
  return dateLong(iso);
}

export function amountText(amount: Amount): string {
  if (amount.currency === 'XTR') {
    return count(amount.minor, 'звезда', 'звезды', 'звёзд');
  }
  return money({ minor: amount.minor, currency: amount.currency });
}

const CRYPTO = new Set(['cryptopay', 'heleket']);
const CARD = new Set(['bank_card', 'card']);

export function lowerFirst(text: string): string {
  if (text.length > 1 && text.charAt(1) !== text.charAt(1).toLowerCase()) {
    return text;
  }
  return text.charAt(0).toLowerCase() + text.slice(1);
}

export function methodText(provider: string, method: string | null): string {
  const m = (method ?? '').toLowerCase();
  if (provider === 'balance') {
    return 'С баланса';
  }
  if (m === 'autorenew') {
    return 'Автопродление с карты';
  }
  if (provider === 'stars') {
    return 'Звёзды Telegram';
  }
  if (provider === 'gram') {
    return 'GRAM';
  }
  if (CRYPTO.has(provider)) {
    return method ? `Криптовалюта, ${method.toUpperCase()}` : 'Криптовалюта';
  }
  if (m === 'sbp') {
    return 'СБП';
  }
  if (CARD.has(m)) {
    return 'Карта';
  }
  if (m === 'yoo_money' || provider === 'yoomoney') {
    return 'ЮMoney';
  }
  if (m === 'sberbank' || m === 'sber_pay') {
    return 'SberPay';
  }
  if (provider === 'cloudpayments' || provider === 'lava' || provider === 'overpay') {
    return 'Карта';
  }
  return method ?? provider;
}

function period(order: OrderBrief): string {
  return periodLabel(order.days) ?? '';
}

function trialName(order: OrderBrief): string {
  const variant = order.tariff_name && order.tariff_name !== 'Пробный период' ? ` «${order.tariff_name}»` : '';
  return `Пробный период${variant}`;
}

function bought(order: OrderBrief | null): string {
  if (!order) {
    return 'Оплата заказа';
  }
  const name = order.tariff_name ? `«${order.tariff_name}»` : 'подписки';
  const span = period(order);
  switch (order.kind) {
    case 'top_up':
      return 'Пополнение баланса';
    case 'change':
      return order.tariff_name ? `Переход на «${order.tariff_name}»` : 'Смена тарифа';
    case 'module':
      return order.label ? `Оплата: ${order.label}` : 'Оплата дополнения';
    case 'trial':
      return trialName(order);
    default:
      return span ? `Оплата ${name} на ${span}` : `Оплата ${name}`;
  }
}

function actorNote(item: Extract<HistoryItem, { type: 'audit' }>): string {
  if (item.actor_kind === 'admin') {
    return item.actor_email ?? 'Администратор';
  }
  if (item.actor_kind === 'client') {
    return 'Сам клиент';
  }
  return 'Автоматически';
}

function num(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function str(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function cut(text: string, max = 90): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

const AUDIT_TEXT: Record<string, string> = {
  'client.note': 'Изменена заметка',
  'client.unblock': 'Разблокировка',
  'client.telegram_login_confirmed': 'Вход в кабинет через Telegram',
  'client.totp_enabled': 'Включена 2FA',
  'client.totp_disabled': 'Выключена 2FA',
  'client.totp_reset': 'Сброшена 2FA',
  'client.password_issued': 'Выдан новый пароль для сайта',
  'client.password_reset': 'Пароль восстановлен по коду из письма',
  'client.password_changed': 'Пароль изменён в кабинете',
  'subscription.reset_traffic': 'Сброшен трафик',
  'subscription.reissue_link': 'Выпущена новая ссылка подписки',
  'subscription.disconnect_all_devices': 'Отключены все устройства',
  'subscription.disconnect_device': 'Отключено устройство',
  'subscription.change_limits': 'Изменены лимиты',
  'subscription.block_access': 'Доступ к подписке закрыт',
  'subscription.restore_access': 'Доступ к подписке открыт',
  'subscription.release_devices': 'Освобождены места для устройств',
  'subscription.blocked_delivery': 'Выдача остановлена: клиент заблокирован',
  'subscription.traffic_restricted': 'Подписка ограничена за превышение трафика',
  'subscription.traffic_lifted': 'Ограничение по трафику снято',
  'payment.mark_paid': 'Платёж отмечен оплаченным вручную',
  'payment.refund': 'Оформлен возврат',
  'payment.refund_requested': 'Запрошен возврат',
  'payment.refund_confirmed': 'Возврат подтверждён',
  'payment.refund_confirmed_manually': 'Возврат подтверждён вручную',
  'payment.refund_declined': 'Возврат отклонён',
  'payment.refund_canceled': 'Возврат отменён',
  'payment.refund_outside': 'Возврат отмечен как сделанный вне панели',
  'payment.underpaid_credited': 'Оплата пришла не полностью, полученное зачислено на баланс',
  'payment.underpaid_review': 'Оплата пришла не полностью, деньги ждут ручного зачисления',
  'payment.overpaid_credited': 'Излишек оплаты зачислен на баланс',
  'payment.overpaid_review': 'Излишек оплаты ждёт ручного зачисления',
  'payment.saved_method_revoked': 'Клиент отозвал автосписание, сохранённая карта снята',
  'order.redeliver': 'Заказ выдан повторно',
  'order.mark_fulfilled': 'Заказ отмечен выданным',
  'order.recalculate': 'Заказ пересчитан',
  'order.review': 'Заказ отправлен на проверку',
  'order.replaced': 'Счёт отменён: промокод перешёл в новый счёт',
  'order.attached': 'Оплата зачтена в текущую подписку: у клиента может быть только одна',
};

const SOURCE_TEXT: Record<string, string> = {
  bot: 'Регистрация через бота',
  miniapp: 'Регистрация через мини-приложение',
  web: 'Регистрация в кабинете',
  admin: 'Создан администратором',
  import: 'Перенос из прежней панели',
  migration: 'Перенос из прежней панели',
};

function personText(person: Person): string {
  if (person.telegram_username) {
    return `@${person.telegram_username}`;
  }
  return person.name ?? `клиент ${person.client_id}`;
}

export function historyLine(item: HistoryItem, index: number, referrer: Person | null): HistoryLine {
  const base = { key: `${item.type}-${item.at}-${index}`, when: historyWhen(item.at) };
  switch (item.type) {
    case 'payment': {
      const { payment, order } = item;
      const what = bought(order);
      const via = methodText(payment.provider, payment.method);
      const sum = amountText(payment.amount);
      if (item.review !== undefined && item.review !== null) {
        const why = str(item.review);
        return { ...base, what: 'Списание с карты ждёт ручной проверки', note: why ? `${via}: ${cut(why, 200)}` : via, sum };
      }
      if (payment.status === 'failed') {
        const reason = str(item.failure_reason);
        return { ...base, what: 'Оплата не прошла', note: reason ? `${what}, ${via}. ${cut(reason, 240)}` : `${what}, ${via}`, sum: '' };
      }
      if (payment.status === 'refunded') {
        return { ...base, what, note: `${via}, деньги вернули`, sum };
      }
      if (payment.status === 'partially_refunded') {
        return { ...base, what, note: `${via}, часть денег вернули`, sum, refund: payment.number };
      }
      if (payment.status === 'paid') {
        return { ...base, what, note: via, sum, refund: payment.number };
      }
      return { ...base, what, note: via, sum };
    }
    case 'order': {
      const { order } = item;
      if (order.kind === 'trial') {
        return { ...base, what: order.days ? `${trialName(order)} на ${days(order.days)}` : trialName(order), note: '', sum: '' };
      }
      const name = order.tariff_name ? `«${order.tariff_name}»` : 'подписка';
      const span = period(order);
      if (order.kind === 'admin_grant') {
        return { ...base, what: span ? `Выдана ${name} на ${span}` : `Выдана ${name}`, note: 'Без оплаты, администратор', sum: '' };
      }
      if (order.kind === 'module_grant') {
        return { ...base, what: order.label ?? (span ? `${name} на ${span}` : name), note: 'Без оплаты', sum: '' };
      }
      return { ...base, what: span ? `${name} на ${span}` : name, note: 'Без оплаты', sum: '' };
    }
    case 'event': {
      const event = item.event;
      switch (item.kind) {
        case 'client_registered': {
          const source = SOURCE_TEXT[String(event.source)] ?? 'Регистрация';
          if (referrer) {
            return { ...base, what: `${source} по приглашению`, note: `Приглашение от ${personText(referrer)}`, sum: '' };
          }
          const mark = startMark(event.start_param);
          const via = startVia(event.start_param);
          return { ...base, what: source, note: mark ? `Метка «${cut(mark, 40)}»` : via ? START_VIA_TEXT[via] : '', sum: '' };
        }
        case 'subscription_status_changed':
          return { ...base, what: event.to === 'limited' ? 'Закончился трафик' : 'Подписка истекла', note: '', sum: '' };
        case 'ticket_opened':
          return { ...base, what: 'Обращение в поддержку', note: item.ticket?.subject ? `«${cut(item.ticket.subject)}»` : '', sum: '' };
        case 'ticket_answered':
          return { ...base, what: 'Поддержка ответила', note: item.ticket?.subject ? `«${cut(item.ticket.subject)}»` : '', sum: '' };
        case 'balance_changed': {
          const delta = event.delta as Money | undefined;
          const plus = (delta?.minor ?? 0) >= 0;
          const kind = String(event.kind);
          const sum = delta ? `${plus ? '+' : ''}${kind.startsWith('topup_bonus') ? moneyExact(delta) : money(delta)}` : '';
          const what =
            kind === 'admin_adjust'
              ? plus
                ? 'Баланс пополнен администратором'
                : 'Списание с баланса администратором'
              : kind === 'referral_credit'
                ? 'Начислено за приглашённого друга'
                : kind === 'refund'
                  ? 'Возврат на баланс'
                  : kind === 'topup_bonus'
                    ? 'Бонус за пополнение'
                    : kind === 'topup_bonus_reversal'
                      ? 'Бонус за пополнение снят'
                      : 'Изменение баланса';
          return { ...base, what, note: event.wallet === 'referral' ? 'Реферальный баланс' : '', sum };
        }
        default:
          return { ...base, what: item.kind, note: '', sum: '' };
      }
    }
    case 'audit': {
      const details = item.details ?? {};
      const actor = actorNote(item);
      switch (item.action) {
        case 'client.receipt_email': {
          const email = str(details.email);
          return { ...base, what: details.changed === true ? 'Изменена почта для чеков' : 'Указана почта для чеков', note: email ? `${email}${actor ? ` · ${actor}` : ''}` : actor, sum: '' };
        }
        case 'subscription.add_days': {
          const n = num(details.days);
          return { ...base, what: n ? `Добавлено ${days(n)}` : 'Добавлены дни', note: actor, sum: '' };
        }
        case 'subscription.renewal_started':
        case 'subscription.renewal_cancel_requested':
        case 'subscription.renewal_cancelled':
        case 'subscription.renewal_failed':
        case 'subscription.renewal_charge_found': {
          const provider = str(details.provider) === 'lava' ? 'Lava.top' : (str(details.provider) ?? 'платёжную систему');
          const source = str(details.source);
          const sources: Record<string, string> = {
            client: 'отменил клиент',
            operator: 'отменил оператор',
            provider: `отменено в ${provider}`,
            refund: 'отменено возвратом',
            tariff: 'отменено при смене тарифа',
            block: 'отменено при блокировке клиента',
            chargeback: 'отменено после чарджбэка',
            gone: 'подписки в панели уже нет',
          };
          const what: Record<string, string> = {
            'subscription.renewal_started': `Оформлено автопродление через ${provider}`,
            'subscription.renewal_cancel_requested': `Отправлена отмена автопродления в ${provider}`,
            'subscription.renewal_cancelled': `Автопродление через ${provider} отменено`,
            'subscription.renewal_failed': `${provider} не смогла списать за продление, автопродление закрыто`,
            'subscription.renewal_charge_found': `Сверка с ${provider} нашла продление без уведомления`,
          };
          const note = source && sources[source] ? sources[source] : actor;
          return { ...base, what: what[item.action] ?? item.action, note, sum: '' };
        }
        case 'subscription.add_days_undone': {
          const n = num(details.days);
          return { ...base, what: n ? `Отменено добавление ${days(n)}` : 'Отменено добавление дней', note: actor, sum: '' };
        }
        case 'client.block': {
          const reason = str(details.reason);
          return { ...base, what: 'Блокировка', note: reason ? `${actor}: ${cut(reason, 70)}` : actor, sum: '' };
        }
        case 'payment.autorenew_unclear':
        case 'payment.autorenew_blocked': {
          const reason = str(details.reason);
          const what = item.action === 'payment.autorenew_unclear' ? 'Исход списания с карты неизвестен' : 'Списание с карты у заблокированного клиента';
          return { ...base, what, note: reason ? cut(reason, 160) : actor, sum: '' };
        }
        case 'order.redeliver': {
          const what = details.recalculated === true ? 'Заказ пересчитан и выдан повторно' : 'Заказ выдан повторно';
          const reason = str(details.reason);
          return { ...base, what, note: reason ? `${actor}: ${cut(reason, 70)}` : actor, sum: '' };
        }
        case 'client.email_changed': {
          const email = str(details.email);
          const previous = str(details.previous);
          const added = details.confirmed === true ? `Почта ${email} подтверждена` : `Добавлена почта ${email}`;
          const what = email ? (previous ? `Почта изменена на ${email}` : added) : 'Почта изменена';
          return { ...base, what, note: previous ? `${actor}, прежняя ${previous}` : actor, sum: '' };
        }
        case 'client.email_linked': {
          const email = str(details.email);
          const previous = str(details.previous);
          const what = email ? `Указана почта ${email} без кода` : 'Почта указана без кода';
          return { ...base, what, note: previous ? `${actor}, прежняя ${previous}` : actor, sum: '' };
        }
        case 'subscription.change_limits': {
          const before = num(details.hwid_limit_before);
          const after = num(details.hwid_limit);
          const devices = before !== null && after !== null && before !== after ? `устройств было ${before}, стало ${after}` : null;
          return { ...base, what: 'Изменены лимиты', note: devices ? `${actor}: ${devices}` : actor, sum: '' };
        }
        case 'client.email_set': {
          const email = str(details.email);
          const previous = str(details.previous);
          const what = email ? (previous ? `Почта изменена администратором на ${email}` : `Почта ${email} указана администратором`) : 'Почта изменена администратором';
          return { ...base, what, note: previous ? `${actor}, прежняя ${previous}` : actor, sum: '' };
        }
        case 'client.email_removed': {
          const email = str(details.email);
          return { ...base, what: email ? `Почта ${email} отвязана администратором` : 'Почта отвязана администратором', note: actor, sum: '' };
        }
        case 'subscription.remna_linked': {
          const name = str(details.remna_username);
          return { ...base, what: name ? `Подписка привязана к пользователю Remnawave ${name}` : 'Подписка привязана к пользователю Remnawave', note: actor, sum: '' };
        }
        case 'subscription.remna_added': {
          const name = str(details.remna_username);
          return { ...base, what: name ? `Добавлена подписка из Remnawave ${name}` : 'Добавлена подписка из Remnawave', note: actor, sum: '' };
        }
        case 'client.message': {
          const text = str(details.text);
          return { ...base, what: 'Отправлено сообщение в бот', note: text ? `${actor}: «${cut(text, 70)}»` : actor, sum: '' };
        }
        default:
          return { ...base, what: AUDIT_TEXT[item.action] ?? `Действие: ${item.action}`, note: actor, sum: '' };
      }
    }
  }
}

export function eventsCount(total: number): string {
  return count(total, 'событие', 'события', 'событий');
}
