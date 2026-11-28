import { test } from 'node:test';
import assert from 'node:assert/strict';
import { FRAME_MAX, FRAME_MIN, frameHeight, withTheme } from './frame.ts';

test('тема админки добавляется к адресу фрейма модуля', () => {
  assert.equal(withTheme('/m/game/cap/card?client_id=7&sn_token=t', 'dark'), '/m/game/cap/card?client_id=7&sn_token=t&sn_theme=dark');
  assert.equal(withTheme('/m/game/cap/', 'light'), '/m/game/cap/?sn_theme=light');
  assert.equal(withTheme('https://mod.example/page?sn_token=t#top', 'dark'), 'https://mod.example/page?sn_token=t&sn_theme=dark#top');
});

test('высота фрейма от модуля держится в пределах', () => {
  assert.equal(frameHeight(321.2), 322);
  assert.equal(frameHeight(0), FRAME_MIN);
  assert.equal(frameHeight(10_000_000), FRAME_MAX);
  assert.equal(frameHeight('200'), null);
  assert.equal(frameHeight(Number.NaN), null);
  assert.equal(frameHeight(Number.POSITIVE_INFINITY), null);
});
