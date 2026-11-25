import type { Lang } from '../../lib/i18n';
import type { Theme } from '../../lib/theme';
import type { Site } from '../../lib/types';
import type { Part } from '../parts';

export const CABINET_PREVIEW = { ready: 'cabinet-preview-ready', draft: 'cabinet-preview-draft', pick: 'cabinet-preview-pick' } as const;

export const PREVIEW_STATES = ['new', 'active', 'ended'] as const;

export type PreviewState = (typeof PREVIEW_STATES)[number];

export const PREVIEW_SCREENS = ['login', 'subscription', 'pay', 'addons', 'devices', 'topup', 'payments', 'friends', 'gifts', 'help', 'ticket', 'security'] as const;

export type PreviewScreen = (typeof PREVIEW_SCREENS)[number];

export const SCREEN_PATHS: Record<Exclude<PreviewScreen, 'login'>, string> = {
  subscription: '/cabinet',
  pay: '/cabinet/pay',
  addons: '/cabinet/addons',
  devices: '/cabinet/devices',
  topup: '/cabinet/topup',
  payments: '/cabinet/payments',
  friends: '/cabinet/friends',
  gifts: '/cabinet/gifts',
  help: '/cabinet/help',
  ticket: '/cabinet/help/1',
  security: '/cabinet/security',
};

export type CabinetDraft = { type: typeof CABINET_PREVIEW.draft; site: Site; css: string; theme: Theme; lang: Lang; state: PreviewState; screen: PreviewScreen; focus: Part | null };
