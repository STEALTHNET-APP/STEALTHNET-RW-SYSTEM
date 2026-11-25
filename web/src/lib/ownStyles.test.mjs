import assert from 'node:assert/strict';
import { test } from 'node:test';
import { styleFile } from './ownStyles.ts';

test('подключается только файл стилей своей страницы', () => {
  const file = '/api/public/miniapp/0123456789abcdef.css';
  assert.equal(styleFile(file, 'miniapp'), file);
  assert.equal(styleFile(file, 'landing'), null);
  assert.equal(styleFile('/api/public/landing/0123456789abcdef.css', 'landing'), '/api/public/landing/0123456789abcdef.css');
  assert.equal(styleFile('/api/public/cabinet/0123456789abcdef.css', 'cabinet'), '/api/public/cabinet/0123456789abcdef.css');
  assert.equal(styleFile(file, 'cabinet'), null);
  assert.equal(styleFile('/api/public/cabinet/0123456789abcdef.css', 'miniapp'), null);
  for (const value of [null, undefined, 5, '', `https://evil.example${file}`, `${file}?x`, '/api/public/miniapp/0123.css', '/api/public/miniapp/../x.css', '/api/public/miniapp/0123456789ABCDEF.css', `${file}\n`]) {
    assert.equal(styleFile(value, 'miniapp'), null, String(value));
  }
});
