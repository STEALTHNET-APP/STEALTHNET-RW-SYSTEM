import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clock, fileProblem, kindOf, megabytes } from './files.ts';

const limits = { image_bytes: 10_000_000, video_bytes: 50_000_000 };

test('вид вложения берётся по типу файла', () => {
  assert.equal(kindOf({ type: 'image/png', name: 'a.png' }), 'photo');
  assert.equal(kindOf({ type: 'image/jpeg', name: 'a.jpg' }), 'photo');
  assert.equal(kindOf({ type: 'video/mp4', name: 'promo.mp4' }), 'video');
  assert.equal(kindOf({ type: 'video/quicktime', name: 'IMG_0001.MOV' }), 'mov');
  assert.equal(kindOf({ type: '', name: 'clip.mov' }), 'mov');
  assert.equal(kindOf({ type: 'image/gif', name: 'a.gif' }), null);
  assert.equal(kindOf({ type: 'video/webm', name: 'a.webm' }), null);
});

test('пределы Telegram: картинка до 10 МБ, видео до 50 МБ', () => {
  assert.equal(fileProblem({ type: 'image/png', name: 'a.png', size: 10_000_000 }, limits, true), null);
  assert.match(fileProblem({ type: 'image/png', name: 'a.png', size: 10_000_001 }, limits, true), /^Картинка больше 10 МБ/);
  assert.equal(fileProblem({ type: 'video/mp4', name: 'a.mp4', size: 50_000_000 }, limits, true), null);
  assert.match(fileProblem({ type: 'video/mp4', name: 'a.mp4', size: 50_000_001 }, limits, true), /^Видео больше 50 МБ/);
  assert.match(fileProblem({ type: 'video/quicktime', name: 'a.mov', size: 10 }, limits, true), /MP4/);
  assert.match(fileProblem({ type: 'image/gif', name: 'a.gif', size: 10 }, limits, true), /JPEG или PNG или видео MP4/);
});

test('в письмо видео не прикладывается', () => {
  assert.match(fileProblem({ type: 'video/mp4', name: 'a.mp4', size: 10 }, limits, false), /только картинку/);
  assert.equal(fileProblem({ type: 'image/jpeg', name: 'a.jpg', size: 10 }, limits, false), null);
});

test('длительность и размер пишутся коротко', () => {
  assert.equal(clock(34), '0:34');
  assert.equal(clock(65), '1:05');
  assert.equal(clock(3725), '1:02:05');
  assert.equal(clock(null), '0:00');
  assert.equal(megabytes(999), '1 КБ');
  assert.equal(megabytes(512_000), '512 КБ');
  assert.equal(megabytes(6_400_000), '6,4 МБ');
  assert.equal(megabytes(10_000_000), '10 МБ');
  assert.equal(megabytes(10_221_896), '10,2 МБ');
  assert.equal(megabytes(49_960_000), '50 МБ');
  assert.equal(megabytes(49_600_000), '49,6 МБ');
});
