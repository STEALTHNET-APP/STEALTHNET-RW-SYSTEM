import { ACCOUNT } from './en-account';
import { CABINET } from './en-cabinet';
import { CABINET2 } from './en-cabinet2';
import { COMMON } from './en-common';
import { CONNECT } from './en-connect';
import { MINI } from './en-mini';
import { OPEN } from './en-open';
import { PROMO } from './en-promo';
import { SALE } from './en-sale';
import { SITE } from './en-site';
import { SUPPORT } from './en-support';
import { VERIFY } from './en-verify';

export const EN: Record<string, string> = { ...COMMON, ...CABINET, ...CABINET2, ...MINI, ...SITE, ...ACCOUNT, ...CONNECT, ...PROMO, ...VERIFY, ...SUPPORT, ...OPEN, ...SALE };

export const PLURALS: Record<string, [string, string]> = {
  день: ['day', 'days'],
  месяц: ['month', 'months'],
  минуту: ['minute', 'minutes'],
  час: ['hour', 'hours'],
  устройство: ['device', 'devices'],
  звезда: ['star', 'stars'],
  оплата: ['payment', 'payments'],
  покупку: ['purchase', 'purchases'],
  вращение: ['spin', 'spins'],
  'вращение есть': ['spin left', 'spins left'],
  'день остался': ['day left', 'days left'],
  пришёл: ['joined', 'joined'],
  друг: ['friend', 'friends'],
  друга: ['friend', 'friends'],
  'новый ответ': ['new reply', 'new replies'],
};
