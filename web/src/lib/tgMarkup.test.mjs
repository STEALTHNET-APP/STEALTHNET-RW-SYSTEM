import { test } from 'node:test';
import assert from 'node:assert/strict';
import { firstParagraph, parseTg, problemText, safeLink, tgLength, tgLine, tgPlain, tgToHtml } from './tgMarkup.ts';

const clean = (html) => tgToHtml(parseTg(html).nodes);
const problem = (html) => {
  const found = parseTg(html).problem;
  return found ? problemText(found) : null;
};

test('разрешённая разметка остаётся как есть', () => {
  const html = '🎉 <b>Тариф:</b> 30 дней\n💰 <b>Цена:</b> <s>от 419 ₽</s> <i>(курсив)</i> <u>подчёркнутый</u> <a href="https://t.me/support">поддержка</a>';
  assert.equal(clean(html), html);
  assert.equal(problem(html), null);
});

test('синонимы тегов сводятся к тегам Telegram', () => {
  assert.equal(clean('<strong>a</strong><em>b</em><ins>c</ins><del>d</del><strike>e</strike>'), '<b>a</b><i>b</i><u>c</u><s>d</s><s>e</s>');
});

test('обычный текст с угловыми скобками и амперсандом не ломается', () => {
  assert.equal(clean('Скорость < 100 & > 5'), 'Скорость &lt; 100 &amp; &gt; 5');
  assert.equal(tgPlain('Скорость < 100 & > 5 <3'), 'Скорость < 100 & > 5 <3');
  assert.equal(problem('Скорость < 100 & > 5 <3'), null);
});

test('старое однострочное описание без разметки выводится тем же текстом', () => {
  const old = 'Для мобильного интернета с белыми списками';
  assert.equal(clean(old), old);
  assert.equal(tgPlain(old), old);
  assert.equal(firstParagraph(parseTg(old).nodes).more, false);
});

test('незакрытый тег закрывается, проверка его находит', () => {
  assert.equal(clean('<b>жирный до конца'), '<b>жирный до конца</b>');
  assert.equal(problem('<b>жирный до конца'), 'тег <b> не закрыт');
});

test('лишний закрывающий тег выбрасывается, проверка его находит', () => {
  assert.equal(clean('лишний</i> текст'), 'лишний текст');
  assert.equal(problem('лишний</i> текст'), 'лишний закрывающий тег </i>');
  assert.equal(clean('<b><i>x</b>y</i>'), '<b><i>x</i></b>y');
});

test('чужие теги показываются текстом, а не разметкой', () => {
  assert.equal(clean('<div>чужой</div><script>alert(1)</script>'), '&lt;div&gt;чужой&lt;/div&gt;&lt;script&gt;alert(1)&lt;/script&gt;');
  assert.equal(problem('<div>чужой</div>'), null);
});

test('ссылка только на http, https и tg', () => {
  assert.equal(clean('<a href="javascript:alert(1)">x</a>'), 'x');
  assert.equal(problem('<a href="javascript:alert(1)">x</a>'), 'у ссылки должен быть адрес вида <a href="https://…">текст</a>');
  assert.equal(clean('<a href="tg://resolve?domain=x">x</a>'), '<a href="tg://resolve?domain=x">x</a>');
  assert.equal(clean('<a href="{link}">x</a>'), 'x');
  assert.equal(safeLink('https://example.com/a?b=1&amp;c=2'), 'https://example.com/a?b=1&c=2');
  assert.equal(safeLink(' data:text/html,x'), null);
});

test('вложенность, которую Telegram не принимает, разворачивается', () => {
  assert.equal(clean('<code><b>x</b></code>'), '<code>x</code>');
  assert.equal(clean('<pre><code class="language-js">x</code></pre>'), '<pre><code class="language-js">x</code></pre>');
  assert.equal(clean('<a href="https://a.b"><a href="https://c.d">in</a></a>'), '<a href="https://a.b">in</a>');
  assert.equal(clean('<blockquote>a<blockquote>b</blockquote></blockquote>'), '<blockquote>ab</blockquote>');
});

test('премиум-эмодзи и скрытый текст сохраняются', () => {
  const html = '<tg-emoji emoji-id="5368324170671202286">🔥</tg-emoji> <span class="tg-spoiler">тайна</span> <tg-spoiler>ещё</tg-spoiler>';
  assert.equal(clean(html), '<tg-emoji emoji-id="5368324170671202286">🔥</tg-emoji> <tg-spoiler>тайна</tg-spoiler> <tg-spoiler>ещё</tg-spoiler>');
  assert.equal(clean('<tg-emoji emoji-id="abc">🔥</tg-emoji>'), '🔥');
  assert.equal(clean('<span style="color:red">x</span>'), 'x');
});

test('сущности раскодируются, длина считается без тегов', () => {
  assert.equal(tgPlain('A &amp; B &lt;3 &#128512; &#x41;'), 'A & B <3 😀 A');
  assert.equal(tgLength('<b>абв</b>'), 3);
  assert.equal(tgLength('📦'), 2);
  assert.equal(tgLine('🎉 <b>Акция</b>\n\n⏳ Два дня'), '🎉 Акция ⏳ Два дня');
});

test('свёрнутый вид — первый абзац с оформлением', () => {
  const html = '🎉 <b>Чёрная пятница!</b>\n⏳ Только 2 дня!\n\n📦 <b>Тариф:</b> 30 дней';
  const head = firstParagraph(parseTg(html).nodes);
  assert.equal(head.more, true);
  assert.equal(tgToHtml(head.nodes), '🎉 <b>Чёрная пятница!</b>\n⏳ Только 2 дня!');
  const split = firstParagraph(parseTg('<b>начало\n\nконец</b>').nodes);
  assert.equal(tgToHtml(split.nodes), '<b>начало</b>');
  assert.equal(firstParagraph(parseTg('один абзац\nвторая строка\n\n   ').nodes).more, false);
});
