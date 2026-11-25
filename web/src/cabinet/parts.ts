export const PARTS = ['header', 'login', 'subscription', 'connect', 'pay', 'devices', 'money', 'friends', 'gifts', 'help', 'security', 'footer'] as const;

export type Part = (typeof PARTS)[number];

export function isPart(value: unknown): value is Part {
  return typeof value === 'string' && (PARTS as readonly string[]).includes(value);
}

const PATHS: Array<[string, Part]> = [
  ['/cabinet/payments', 'money'],
  ['/cabinet/topup', 'money'],
  ['/cabinet/addons', 'pay'],
  ['/cabinet/pay', 'pay'],
  ['/cabinet/security', 'security'],
  ['/cabinet/devices', 'devices'],
  ['/cabinet/friends', 'friends'],
  ['/cabinet/gifts', 'gifts'],
  ['/cabinet/help', 'help'],
];

export function partOf(pathname: string): Part {
  return PATHS.find(([path]) => pathname === path || pathname.startsWith(`${path}/`))?.[1] ?? 'subscription';
}
