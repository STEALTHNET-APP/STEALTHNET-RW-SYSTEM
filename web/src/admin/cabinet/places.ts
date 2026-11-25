import type { Part } from '../../cabinet/parts';
import type { PreviewScreen, PreviewState } from '../../cabinet/preview/model';
import { snippet, type Snippet } from '../css/cssText';

export type CabinetPlace = 'page' | Part;

export const CABINET_PLACES: CabinetPlace[] = ['page', 'header', 'login', 'subscription', 'connect', 'pay', 'devices', 'money', 'friends', 'gifts', 'help', 'security', 'footer'];

export const CSS_PATH = '/api/admin/settings/cabinet/css';

export const PLACE_NAMES: Record<CabinetPlace, string> = {
  page: 'Общий CSS',
  header: 'Шапка и меню',
  login: 'Вход и регистрация',
  subscription: 'Подписка',
  connect: 'Подключение',
  pay: 'Тарифы и оплата',
  devices: 'Устройства',
  money: 'Пополнение и история оплат',
  friends: 'Друзья',
  gifts: 'Подарки',
  help: 'Помощь',
  security: 'Безопасность',
  footer: 'Ссылки внизу',
};

export const PLACE_ABOUT: Record<CabinetPlace, string> = {
  page: 'Весь кабинет: цвета, шрифт, кнопки',
  header: 'Логотип, вкладки, баланс, профиль, меню на телефоне',
  login: 'Экран до входа: Telegram, почта, новый пароль',
  subscription: 'Главная: срок, баланс, автопродление',
  connect: 'Карточка на главной: ссылка, QR-код, шаги',
  pay: 'Выбор тарифа, срок, способ оплаты, докупка',
  devices: 'Страница «Устройства»',
  money: 'Суммы, способы, список платежей',
  friends: 'Ссылки, итоги, вывод',
  gifts: 'Покупка, коды, активация',
  help: 'Новое обращение и переписка',
  security: 'Способы входа, почта, пароль, код',
  footer: 'Помощь и документы под страницей',
};

export function placeTitle(place: CabinetPlace): string {
  return place === 'page' ? 'Общий CSS' : `${PLACE_NAMES[place]}, CSS раздела`;
}

export function placeHint(place: CabinetPlace): string {
  return place === 'page'
    ? 'Действует на весь кабинет, и на вход с оплатой тоже. Лендинг, мини-приложение и админка не изменятся'
    : place === 'subscription'
      ? 'Действует только на главную кабинета, кроме карточки «Подключение»: у неё своё место'
      : `Действует только на раздел «${PLACE_NAMES[place]}»`;
}

export const SCREEN_NAMES: Array<{ key: PreviewScreen; name: string }> = [
  { key: 'subscription', name: 'Подписка' },
  { key: 'login', name: 'Вход' },
  { key: 'pay', name: 'Тарифы' },
  { key: 'addons', name: 'Докупка' },
  { key: 'devices', name: 'Устройства' },
  { key: 'topup', name: 'Пополнение' },
  { key: 'payments', name: 'История оплат' },
  { key: 'friends', name: 'Друзья' },
  { key: 'gifts', name: 'Подарки' },
  { key: 'help', name: 'Помощь' },
  { key: 'ticket', name: 'Обращение' },
  { key: 'security', name: 'Безопасность' },
];

export const STATES: Array<{ key: PreviewState; name: string }> = [
  { key: 'new', name: 'новый' },
  { key: 'active', name: 'с подпиской' },
  { key: 'ended', name: 'подписка закончилась' },
];

export function screenOf(place: CabinetPlace, current: PreviewScreen): PreviewScreen {
  switch (place) {
    case 'page':
      return current;
    case 'header':
    case 'footer':
      return current === 'login' ? 'subscription' : current;
    case 'connect':
      return 'subscription';
    case 'pay':
      return current === 'addons' ? current : 'pay';
    case 'money':
      return current === 'payments' ? current : 'topup';
    case 'help':
      return current === 'ticket' ? current : 'help';
    default:
      return place;
  }
}

export function placeOf(screen: PreviewScreen): CabinetPlace {
  switch (screen) {
    case 'addons':
      return 'pay';
    case 'topup':
    case 'payments':
      return 'money';
    case 'ticket':
      return 'help';
    default:
      return screen;
  }
}

const SAMPLES: Record<CabinetPlace, string> = {
  page: 'body {\n  --accent: #0f766e;\n}',
  header: '.cb-tab {\n  border-radius: 20px;\n}',
  login: 'h1 {\n  font-size: 26px;\n}',
  subscription: '.cb-days {\n  color: #0f766e;\n}',
  connect: '.cb-step {\n  border-radius: 14px;\n}',
  pay: '.cb-term-card {\n  border-radius: 20px;\n}',
  devices: '.cb-card {\n  border-radius: 22px;\n}',
  money: '.cb-amount {\n  border-radius: 20px;\n}',
  friends: '.cb-link {\n  font-size: 14px;\n}',
  gifts: '.cb-gift-code {\n  color: #0f766e;\n}',
  help: '.cb-msg {\n  background: #e6f4f1;\n}',
  security: '.cb-card {\n  border-radius: 22px;\n}',
  footer: 'a {\n  color: #0f766e;\n}',
};

export function placeSample(place: CabinetPlace): string {
  return SAMPLES[place];
}

const COMMON: Snippet[] = [snippet('Весь раздел', '&'), snippet('Карточки', '.cb-card'), snippet('Заголовок раздела', '.cb-title'), snippet('Главные кнопки', '.cb-main'), snippet('Обычные кнопки', '.cb-second')];

export const SNIPPETS: Record<CabinetPlace, Snippet[]> = {
  page: [
    { name: 'Цвета всего кабинета', css: 'body {\n  --accent: #0f766e;\n  --accent-ink: #0f766e;\n  --accent-soft: color-mix(in srgb, #0f766e 12%, transparent);\n  --btn: #0f766e;\n  --btn-ink: #ffffff;\n}' },
    { name: 'Фон', css: 'body {\n  --bg: #f4f7f6;\n  --surface: #ffffff;\n}' },
    snippet('Шрифт', 'body'),
    snippet('Карточки', '.cb-card'),
    snippet('Заголовки страниц', '.cb-title'),
    snippet('Крупные заголовки', '.cb-h1'),
    snippet('Заголовки карточек', '.cb-h2'),
    snippet('Главные кнопки', '.cb-main'),
    snippet('Обычные кнопки', '.cb-second'),
    snippet('Ссылки-кнопки', '.cb-accent-link'),
    snippet('Поля ввода', '.cb-field input'),
    snippet('Переключатели', '.cb-seg button'),
    snippet('Окно входа', '.cb-solo-box'),
    snippet('Тёмная тема', "[data-theme='dark'] body"),
    { name: 'Узкий телефон', css: '@media (max-width: 360px) {\n  .cb {\n    \n  }\n}' },
  ],
  header: [
    snippet('Вся шапка', '&'),
    snippet('Логотип', '.cb-mark'),
    snippet('Название', '.cb-brand-name'),
    snippet('Вкладка', '.cb-tab'),
    snippet('Открытая вкладка', ".cb-tab[aria-current='page']"),
    snippet('Баланс', '.cb-balance'),
    snippet('Кнопка профиля', '.cb-ava'),
    snippet('Меню профиля', '.cb-pop'),
    snippet('Меню на телефоне', '.cb-menu'),
    snippet('Пункт меню на телефоне', '.cb-menu-item'),
  ],
  login: [
    snippet('Весь экран', '&'),
    snippet('Окно входа', '.cb-solo-box'),
    snippet('Заголовок', 'h1'),
    snippet('Переключатель Telegram и почты', '.cb-seg button'),
    snippet('Выбранный способ', ".cb-seg button[aria-pressed='true']"),
    snippet('Поля ввода', '.cb-field input'),
    snippet('Главная кнопка', '.cb-main'),
    snippet('Подсказки', '.cb-login-help'),
  ],
  subscription: [
    snippet('Вся главная', '&'),
    snippet('Карточка подписки', '.cb-sub'),
    snippet('Название тарифа', '.cb-sub .cb-h1'),
    snippet('Число дней', '.cb-days'),
    snippet('Полоса срока', '.cb-bar'),
    snippet('Заполненная часть полосы', '.cb-bar span'),
    snippet('Состояние', '.cb-status'),
    snippet('Баланс', '.cb-balance-card'),
    snippet('Сумма баланса', '.cb-sum'),
    snippet('Автопродление', '.cb-auto-row'),
    snippet('Друзья', '.cb-friends'),
    snippet('Помощь', '.cb-help'),
    snippet('Главные кнопки', '.cb-main'),
    snippet('Обычные кнопки', '.cb-second'),
  ],
  connect: [
    snippet('Вся карточка', '&'),
    snippet('Заголовок', '.cb-h2'),
    snippet('Выбор устройства', '.cb-plats button'),
    snippet('Выбранное устройство', ".cb-plats button[aria-pressed='true']"),
    snippet('Шаг', '.cb-step'),
    snippet('Номер шага', '.cb-num'),
    snippet('Текст шага', '.cb-step-text'),
    snippet('Главная кнопка', '.cb-main'),
  ],
  pay: [
    snippet('Весь раздел', '&'),
    snippet('Карточки', '.cb-card'),
    snippet('Выбор тарифа', '.cb-cats button'),
    snippet('Срок', '.cb-term-card'),
    snippet('Выбранный срок', ".cb-term-card[aria-checked='true']"),
    snippet('Цена срока', '.cb-term-price'),
    snippet('Выгода', '.cb-term-save'),
    snippet('Способ оплаты', '.cb-method'),
    snippet('Выбранный способ', ".cb-method[aria-checked='true']"),
    snippet('Итого', '.cb-total'),
    snippet('Сумма к оплате', '.cb-due-sum'),
    snippet('Кнопка оплаты', '.cb-main'),
  ],
  devices: [...COMMON, snippet('Устройство', '.cb-device'), snippet('Название устройства', '.cb-device-text strong')],
  money: [
    ...COMMON,
    snippet('Сумма пополнения', '.cb-amount'),
    snippet('Выбранная сумма', ".cb-amount[aria-checked='true']"),
    snippet('Способ оплаты', '.cb-method'),
    snippet('Строка платежа', '.cb-payrow'),
    snippet('Сумма платежа', '.cb-payrow-side strong'),
  ],
  friends: [...COMMON, snippet('Ссылка', '.cb-link'), snippet('Итоги', '.cb-facts'), snippet('Вывод', '.cb-withdraw')],
  gifts: [...COMMON, snippet('Срок', '.cb-term-card'), snippet('Способ оплаты', '.cb-method'), snippet('Мои подарки', '.cb-giftrow'), snippet('Код подарка', '.cb-gift-code')],
  help: [...COMMON, snippet('Обращение в списке', '.cb-payrow'), snippet('Сообщение', '.cb-msg'), snippet('Своё сообщение', '.cb-msg.mine'), snippet('Поле ответа', '.cb-field textarea')],
  security: [...COMMON, snippet('Способ входа', '.cb-device'), snippet('Поля ввода', '.cb-field input'), snippet('Состояние', '.cb-status')],
  footer: [snippet('Вся полоса', '&'), snippet('Ссылки', 'a')],
};
