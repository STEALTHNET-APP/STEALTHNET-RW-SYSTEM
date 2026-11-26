import { test } from 'node:test';
import assert from 'node:assert/strict';
import { emojiIdProblem, insertAt, premiumCount, readPasted } from './emoji.ts';

const reply = [
  'Для текста — скопируйте строку целиком и вставьте в текст:',
  '<tg-emoji emoji-id="111">🔥</tg-emoji> Скидка до &lt;пятницы&gt;',
  '',
  'Для значка кнопки — код в поле «Премиум-эмодзи» у кнопки:',
  '🔥 — 111',
].join('\n');

test('из ответа бота берётся строка для текста', () => {
  assert.deepEqual(readPasted(reply), { kind: 'text', value: '<tg-emoji emoji-id="111">🔥</tg-emoji> Скидка до &lt;пятницы&gt;' });
  assert.deepEqual(readPasted('  <tg-emoji emoji-id="5368324170671202286">🔥</tg-emoji>  '), {
    kind: 'text',
    value: '<tg-emoji emoji-id="5368324170671202286">🔥</tg-emoji>',
  });
  assert.deepEqual(readPasted('&lt;tg-emoji emoji-id=&quot;7&quot;&gt;😎&lt;/tg-emoji&gt;'), {
    kind: 'text',
    value: '<tg-emoji emoji-id="7">😎</tg-emoji>',
  });
});

test('код без тега — это значок кнопки, обычный текст не подходит', () => {
  assert.deepEqual(readPasted(' 5368324170671202286 '), { kind: 'code', value: '5368324170671202286' });
  assert.deepEqual(readPasted('просто текст 🔥'), { kind: 'none' });
  assert.deepEqual(readPasted('<tg-emoji emoji-id="abc">🔥</tg-emoji>'), { kind: 'none' });
});

test('вставка встаёт на место курсора или выделения', () => {
  assert.deepEqual(insertAt('Привет мир', 7, 7, '🔥 '), { text: 'Привет 🔥 мир', caret: 10 });
  assert.deepEqual(insertAt('Привет мир', 7, 10, 'всем'), { text: 'Привет всем', caret: 11 });
  assert.deepEqual(insertAt('abc', 99, 120, '!'), { text: 'abc!', caret: 4 });
});

test('счёт премиум-эмодзи и проверка кода значка', () => {
  assert.equal(premiumCount('a <tg-emoji emoji-id="1">🔥</tg-emoji> b <tg-emoji emoji-id="2">😎</tg-emoji>'), 2);
  assert.equal(premiumCount('без эмодзи'), 0);
  assert.equal(emojiIdProblem(''), null);
  assert.equal(emojiIdProblem(' 123 '), null);
  assert.ok(emojiIdProblem('🔥'));
  assert.ok(emojiIdProblem('1'.repeat(33)));
});
