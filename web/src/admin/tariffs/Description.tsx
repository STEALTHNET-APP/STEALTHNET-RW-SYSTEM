import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { api } from '../../lib/api';
import { escapeTg, parseTg, problemText, safeLink, tgLength } from '../../lib/tgMarkup';
import { editorHtml, fillEditor, linkAt, pastedHtml, tgFragment } from './tgDom';

export const DESCRIPTION_MAX = 2000;
export const DESCRIPTION_RAW_MAX = 8000;
const CAPTION_MAX = 1024;

type Mark = 'b' | 'i' | 'u' | 's';

const MARKS: Array<{ mark: Mark; label: string; title: string; command: string; keys: string }> = [
  { mark: 'b', label: 'Ж', title: 'Жирный', command: 'bold', keys: 'Ctrl+B' },
  { mark: 'i', label: 'К', title: 'Курсив', command: 'italic', keys: 'Ctrl+I' },
  { mark: 'u', label: 'Ч', title: 'Подчёркнутый', command: 'underline', keys: 'Ctrl+U' },
  { mark: 's', label: 'З', title: 'Зачёркнутый', command: 'strikeThrough', keys: 'Ctrl+Shift+X' },
];

const PLACEHOLDER = 'Например: что входит в тариф, для чего он подходит, условия акции';

export function descriptionProblem(value: string): string | null {
  const parsed = parseTg(value);
  if (parsed.problem) {
    return problemText(parsed.problem);
  }
  if (tgLength(value) > DESCRIPTION_MAX) {
    return `описание длиннее ${DESCRIPTION_MAX} знаков`;
  }
  return value.trim().length > DESCRIPTION_RAW_MAX ? `слишком много оформления: вместе с разметкой описание длиннее ${DESCRIPTION_RAW_MAX} знаков` : null;
}

function captionNote(value: string, name: string): string | null {
  const length = tgLength(value) + name.length + 120;
  return length > CAPTION_MAX ? 'С картинкой Telegram показывает до 1024 знаков: если у раздела «Покупка» в боте есть картинка, экран этого тарифа уйдёт без неё' : null;
}

function markOf(event: ReactKeyboardEvent): Mark | 'a' | null {
  if (!(event.metaKey || event.ctrlKey) || event.altKey) {
    return null;
  }
  const key = event.key.toLowerCase();
  if (event.shiftKey) {
    return key === 'x' || key === 'ч' ? 's' : null;
  }
  if (key === 'b' || key === 'и') {
    return 'b';
  }
  if (key === 'i' || key === 'ш') {
    return 'i';
  }
  if (key === 'u' || key === 'г') {
    return 'u';
  }
  return key === 'k' || key === 'л' ? 'a' : null;
}

function escapeAttr(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

type LinkDraft = { href: string; editing: boolean; error: string | null };

type Props = { value: string; name: string; onChange: (value: string) => void; disabled: boolean };

export function DescriptionField({ value, name, onChange, disabled }: Props) {
  const id = useId();
  const box = useRef<HTMLDivElement | null>(null);
  const emitted = useRef<string | null>(null);
  const range = useRef<Range | null>(null);
  const [mode, setMode] = useState<'visual' | 'code'>('visual');
  const [marks, setMarks] = useState<Record<Mark, boolean>>({ b: false, i: false, u: false, s: false });
  const [link, setLink] = useState<LinkDraft | null>(null);
  const [empty, setEmpty] = useState(!value.trim());
  const [working, setWorking] = useState(false);
  const problem = descriptionProblem(value);
  const note = problem ? null : captionNote(value, name);
  const length = tgLength(value);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el || mode !== 'visual' || value === emitted.current) {
      return;
    }
    fillEditor(el, value);
    emitted.current = value;
    setEmpty(!(el.textContent ?? '').trim());
  }, [value, mode]);

  const save = () => {
    const el = box.current;
    const selection = document.getSelection();
    if (!el || !selection || selection.rangeCount === 0 || !el.contains(selection.getRangeAt(0).commonAncestorContainer)) {
      return;
    }
    range.current = selection.getRangeAt(0).cloneRange();
    setMarks({
      b: document.queryCommandState('bold'),
      i: document.queryCommandState('italic'),
      u: document.queryCommandState('underline'),
      s: document.queryCommandState('strikeThrough'),
    });
  };

  useEffect(() => {
    document.addEventListener('selectionchange', save);
    return () => document.removeEventListener('selectionchange', save);
  }, []);

  const emit = () => {
    const el = box.current;
    if (!el) {
      return;
    }
    const next = editorHtml(el);
    emitted.current = next;
    setEmpty(!(el.textContent ?? '').trim());
    onChange(next);
  };

  const restore = () => {
    const el = box.current;
    if (!el) {
      return;
    }
    const saved = range.current;
    const selection = document.getSelection();
    el.focus();
    if (saved && selection && el.contains(saved.commonAncestorContainer)) {
      selection.removeAllRanges();
      selection.addRange(saved);
    }
  };

  const run = (command: string, arg?: string) => {
    restore();
    document.execCommand('styleWithCSS', false, 'false');
    document.execCommand(command, false, arg);
    emit();
    save();
  };

  const openLink = () => {
    save();
    const el = box.current;
    const anchor = el ? linkAt(range.current?.startContainer ?? null, el) : null;
    setLink({ href: anchor?.getAttribute('href') ?? 'https://', editing: anchor !== null, error: null });
  };

  const applyLink = () => {
    if (!link) {
      return;
    }
    const href = safeLink(link.href);
    if (!href) {
      setLink({ ...link, error: 'Нужна ссылка вида https://… или tg://…' });
      return;
    }
    setLink(null);
    restore();
    const el = box.current;
    const selection = document.getSelection();
    const anchor = el && selection ? linkAt(selection.anchorNode, el) : null;
    if (anchor) {
      anchor.setAttribute('href', href);
    } else if (selection && selection.isCollapsed) {
      document.execCommand('insertHTML', false, tgFragment(`<a href="${escapeAttr(href)}">${escapeAttr(href)}</a>`));
    } else {
      document.execCommand('createLink', false, href);
    }
    emit();
  };

  const removeLink = () => {
    restore();
    const el = box.current;
    const selection = document.getSelection();
    const anchor = el && selection ? linkAt(selection.anchorNode, el) : null;
    if (anchor && selection) {
      const whole = document.createRange();
      whole.selectNodeContents(anchor);
      selection.removeAllRanges();
      selection.addRange(whole);
    }
    document.execCommand('unlink');
    setLink(null);
    emit();
  };

  const paste = (rich: string, plain: string) => {
    const el = box.current;
    const selection = document.getSelection();
    const anchor = el && selection && selection.rangeCount > 0 && el.contains(selection.getRangeAt(0).commonAncestorContainer) ? selection.getRangeAt(0).cloneRange() : null;
    const cleaned = rich
      ? api<{ html: string }>('/api/admin/tariffs/description/clean', { method: 'POST', body: { html: rich } })
          .then((view) => pastedHtml(view.html, plain))
          .catch(() => escapeTg(plain.replace(/\r\n?/g, '\n')))
      : Promise.resolve(escapeTg(plain.replace(/\r\n?/g, '\n')));
    setWorking(true);
    void cleaned.then((html) => {
      setWorking(false);
      const target = box.current;
      if (!target) {
        return;
      }
      target.focus();
      const current = document.getSelection();
      if (anchor && current) {
        current.removeAllRanges();
        current.addRange(anchor);
      }
      if (html) {
        document.execCommand('insertHTML', false, tgFragment(html));
      }
      emit();
    });
  };

  const toVisual = () => {
    if (mode === 'visual') {
      return;
    }
    emitted.current = null;
    setMode('visual');
    window.requestAnimationFrame(() => {
      if (box.current && descriptionProblem(value) !== null) {
        emit();
      }
    });
  };

  const toCode = () => {
    if (mode === 'code') {
      return;
    }
    setLink(null);
    emitted.current = null;
    setMode('code');
  };

  const onKey = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
      event.preventDefault();
      document.execCommand('insertLineBreak');
      emit();
      return;
    }
    const mark = markOf(event);
    if (!mark) {
      return;
    }
    event.preventDefault();
    if (mark === 'a') {
      openLink();
      return;
    }
    run(MARKS.find((m) => m.mark === mark)?.command ?? 'bold');
  };

  return (
    <div className="tf-desc">
      <div className="tf-desc-top">
        <span id={`${id}-label`} className="tf-desc-label">
          Описание для клиента
        </span>
        <div role="toolbar" aria-label="Оформление описания" className="tf-desc-tools">
          {mode === 'visual'
            ? MARKS.map((m) => (
                <button
                  key={m.mark}
                  type="button"
                  className={`tf-tool ${m.mark}`}
                  title={`${m.title} (${m.keys})`}
                  aria-label={m.title}
                  aria-pressed={marks[m.mark]}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => run(m.command)}
                  disabled={disabled}
                >
                  {m.label}
                </button>
              ))
            : null}
          {mode === 'visual' ? (
            <button type="button" className="tf-tool" title="Ссылка (Ctrl+K)" aria-expanded={link !== null} onMouseDown={(e) => e.preventDefault()} onClick={openLink} disabled={disabled}>
              Ссылка
            </button>
          ) : null}
          <div role="group" aria-label="Вид поля" className="segs tf-desc-mode">
            <button type="button" aria-pressed={mode === 'visual'} onClick={toVisual} disabled={disabled}>
              Текст
            </button>
            <button type="button" aria-pressed={mode === 'code'} onClick={toCode} disabled={disabled}>
              Разметка
            </button>
          </div>
        </div>
      </div>
      {link ? (
        <div className="tf-desc-link" role="group" aria-label="Ссылка">
          <input
            className="field mono"
            value={link.href}
            inputMode="url"
            aria-label="Адрес ссылки"
            aria-invalid={link.error !== null}
            autoFocus
            onChange={(e) => setLink({ ...link, href: e.target.value, error: null })}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                applyLink();
              } else if (e.key === 'Escape') {
                setLink(null);
              }
            }}
          />
          <button type="button" className="b36 hov" onClick={applyLink}>
            {link.editing ? 'Изменить' : 'Добавить'}
          </button>
          {link.editing ? (
            <button type="button" className="b36 hov" onClick={removeLink}>
              Убрать ссылку
            </button>
          ) : null}
          <button type="button" className="tf-desc-link-close hov" onClick={() => setLink(null)}>
            Отмена
          </button>
          {link.error ? <span className="err">{link.error}</span> : null}
        </div>
      ) : null}
      {mode === 'visual' ? (
        <div
          ref={box}
          className={empty ? 'field tf-desc-visual is-empty' : 'field tf-desc-visual'}
          contentEditable={!disabled}
          aria-busy={working}
          suppressContentEditableWarning
          role="textbox"
          aria-multiline="true"
          aria-labelledby={`${id}-label`}
          aria-describedby={`${id}-meta`}
          aria-invalid={problem !== null}
          data-placeholder={PLACEHOLDER}
          spellCheck
          onInput={() => {
            emit();
            save();
          }}
          onKeyDown={onKey}
          onKeyUp={save}
          onMouseUp={save}
          onPaste={(event) => {
            event.preventDefault();
            paste(event.clipboardData.getData('text/html'), event.clipboardData.getData('text/plain'));
          }}
          onDrop={(event) => event.preventDefault()}
        />
      ) : (
        <textarea
          className="field tf-desc-text"
          rows={Math.min(16, Math.max(5, value.split('\n').length + 1))}
          value={value}
          aria-labelledby={`${id}-label`}
          aria-describedby={`${id}-meta`}
          aria-invalid={problem !== null}
          spellCheck={false}
          onChange={(e) => onChange(e.target.value)}
          disabled={disabled}
        />
      )}
      <span className="tf-desc-meta">
        <span id={`${id}-meta`} className={problem ? 'err' : 'cap'}>
          {problem ?? note ?? (mode === 'visual' ? 'Переносы строк и пустые строки сохраняются в боте и на сайте' : 'Разметка Telegram: <b>, <i>, <u>, <s>, <a href="https://…">')}
        </span>
        <span className={length > DESCRIPTION_MAX ? 'err num' : 'cap num'}>
          {length} / {DESCRIPTION_MAX}
        </span>
      </span>
    </div>
  );
}
