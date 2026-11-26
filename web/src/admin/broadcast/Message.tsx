import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api, upload } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { displayName, type ClientRow } from '../clients/types';
import { unclosedTag, visibleLength } from '../pages/telegram';
import { BASE, CHANNELS, count, failText, hasBot, hasEmail, imageUrl, type ButtonSpec, type Channel, type Image, type Options, type Section } from './model';
import { BUTTON_STYLES, emojiIdProblem, insertAt, readPasted, type ButtonStyle } from './emoji';
import '../form.css';

type Kind = 'url' | 'section';
export type ButtonDraft = { text: string; kind: Kind; url: string; section: Section; style: ButtonStyle | ''; emoji_id: string };
type Target = { id: number; name: string; handle: string };
export type Note = { text: string; tone: 'ok' | 'bad' | 'plain' };

const TEST_KEY = 'sn.broadcast.test';
const MAIL_KEY = 'sn.broadcast.mailtest';

function stored(key: string): string {
  try {
    return window.localStorage.getItem(key) ?? '';
  } catch {
    return '';
  }
}

function keep(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    return;
  }
}
const URL_START = /^(https?:\/\/[^\s/?#]+|tg:\/\/\S+|t\.me\/\S+|@\w+)/i;

export function toDraft(button: ButtonSpec): ButtonDraft {
  const look = { style: button.style ?? '', emoji_id: button.emoji_id ?? '' } as const;
  return button.section
    ? { text: button.text, kind: 'section', url: '', section: button.section, ...look }
    : { text: button.text, kind: 'url', url: button.url ?? '', section: 'menu', ...look };
}

export function toSpec(button: ButtonDraft): ButtonSpec {
  const base: ButtonSpec = button.kind === 'section' ? { text: button.text, section: button.section } : { text: button.text, url: button.url };
  if (button.style) {
    base.style = button.style;
  }
  if (button.emoji_id.trim()) {
    base.emoji_id = button.emoji_id.trim();
  }
  return base;
}

function storedTarget(): Target | null {
  try {
    const raw = window.localStorage.getItem(TEST_KEY);
    if (!raw) {
      return null;
    }
    const value = JSON.parse(raw) as Target;
    return typeof value.id === 'number' && typeof value.name === 'string' ? value : null;
  } catch {
    return null;
  }
}

function storeTarget(target: Target | null) {
  try {
    if (target) {
      window.localStorage.setItem(TEST_KEY, JSON.stringify(target));
    } else {
      window.localStorage.removeItem(TEST_KEY);
    }
  } catch {
    return;
  }
}

function handleOf(row: ClientRow): string {
  if (row.telegram_username) {
    return `@${row.telegram_username}`;
  }
  if (row.telegram_id) {
    return `Telegram ${row.telegram_id}`;
  }
  return row.email ?? '';
}

function fileSize(bytes: number): string {
  if (bytes < 1024 * 1024) {
    return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
  }
  return `${(bytes / 1024 / 1024).toFixed(1).replace('.', ',')} МБ`;
}

function buttonProblem(button: ButtonDraft, max: number): string | null {
  const text = button.text.trim();
  if (!text) {
    return 'Напишите надпись';
  }
  if ([...text].length > max) {
    return `Надпись длиннее ${max} знаков`;
  }
  if (button.kind === 'url') {
    const url = button.url.trim();
    if (!url) {
      return 'Укажите адрес ссылки';
    }
    if (!URL_START.test(url) || /\s/.test(url)) {
      return 'Адрес должен начинаться с https://, http:// или tg://';
    }
  }
  return emojiIdProblem(button.emoji_id);
}

export type MessageValue = { channel: Channel; subject: string; text: string; image: Image | null; buttons: ButtonDraft[] };

type MessageSource = { channel: Channel; subject: string | null; text: string; image: Image | null; buttons: ButtonSpec[] };

export function messageFrom(source: MessageSource | null): MessageValue {
  return source
    ? { channel: source.channel, subject: source.subject ?? '', text: source.text, image: source.image, buttons: source.buttons.map(toDraft) }
    : { channel: 'bot', subject: '', text: '', image: null, buttons: [] };
}

export function subjectOf(value: MessageValue): string | null {
  const clean = value.subject.trim().replace(/\s+/g, ' ');
  return hasEmail(value.channel) || clean ? clean : null;
}

export type MessageCheck = {
  limit: number;
  length: number;
  markup: string | null;
  textProblem: string | null;
  buttonProblems: Array<string | null>;
  subjectLength: number;
  subjectProblem: string | null;
  messageOk: boolean;
  contentOk: boolean;
  mailOff: boolean;
};

export function checkMessage(value: MessageValue, options: Options, tried: boolean): MessageCheck {
  const limits = options.limits;
  const { channel, subject, text, image, buttons } = value;
  const limit = image ? limits.caption : limits.text;
  const length = visibleLength(text);
  const markup = text.trim() ? unclosedTag(text) : null;
  const textProblem = !text.trim()
    ? tried
      ? 'Напишите текст сообщения'
      : null
    : markup
      ? `Разметка: ${markup}`
      : length > limit
        ? image
          ? `С картинкой Telegram показывает до ${limit} знаков. Сократите текст или уберите картинку`
          : `Telegram показывает до ${limit} знаков`
        : null;
  const buttonProblems = buttons.map((b) => buttonProblem(b, limits.button_text));
  const subjectLength = [...subject.trim()].length;
  const subjectProblem = !hasEmail(channel)
    ? null
    : !subject.trim()
      ? tried
        ? 'Напишите тему письма'
        : null
      : subjectLength > limits.subject
        ? `Тема длиннее ${limits.subject} знаков`
        : null;
  const subjectOk = !hasEmail(channel) || (subject.trim() !== '' && subjectLength <= limits.subject);
  const messageOk = text.trim() !== '' && !markup && length <= limit && buttonProblems.every((p) => p === null);
  return {
    limit,
    length,
    markup,
    textProblem,
    buttonProblems,
    subjectLength,
    subjectProblem,
    messageOk,
    contentOk: messageOk && subjectOk,
    mailOff: hasEmail(channel) && !options.mail.ready,
  };
}

type FieldsProps = {
  options: Options;
  value: MessageValue;
  onChange: (patch: Partial<MessageValue>) => void;
  check: MessageCheck;
  tried: boolean;
  busy: string | null;
  setBusy: (busy: string | null) => void;
  onNote: (note: Note | null) => void;
  preview: ReactNode;
  placeholder: string;
  subjectPlaceholder: string;
  extra?: ReactNode;
};

export function MessageFields({ options, value, onChange, check, tried, busy, setBusy, onNote, preview, placeholder, subjectPlaceholder, extra }: FieldsProps) {
  const limits = options.limits;
  const { channel, subject, text, image, buttons } = value;
  const { limit, length, textProblem, buttonProblems, subjectLength, subjectProblem, mailOff } = check;
  const area = useRef<HTMLTextAreaElement>(null);
  const file = useRef<HTMLInputElement>(null);
  const [emojiOpen, setEmojiOpen] = useState(false);
  const [pasted, setPasted] = useState('');
  const [pasteNote, setPasteNote] = useState<Note | null>(null);
  const caret = useRef<{ start: number; end: number } | null>(null);
  const bot = options.bot_username ? options.bot_username.replace(/^@/, '') : null;

  const remember = () => {
    const el = area.current;
    if (el) {
      caret.current = { start: el.selectionStart, end: el.selectionEnd };
    }
  };

  const pasteEmoji = (raw: string) => {
    const found = readPasted(raw);
    if (found.kind === 'code') {
      setPasteNote({ text: 'Это код для значка кнопки: вставьте его в поле «Премиум-эмодзи» у кнопки ниже', tone: 'plain' });
      return;
    }
    if (found.kind === 'none') {
      setPasteNote({ text: 'Здесь нет премиум-эмодзи. Отправьте боту /emoji вместе с ними и скопируйте строку из ответа', tone: 'bad' });
      return;
    }
    const el = area.current;
    const spot = caret.current ?? { start: text.length, end: text.length };
    const gap = spot.start > 0 && !/\s$/.test(text.slice(0, spot.start)) ? ' ' : '';
    const placed = insertAt(text, spot.start, spot.end, gap + found.value);
    caret.current = { start: placed.caret, end: placed.caret };
    onChange({ text: placed.text });
    setPasted('');
    setPasteNote({ text: 'Вставлено в текст', tone: 'ok' });
    window.requestAnimationFrame(() => {
      if (el) {
        el.focus();
        el.setSelectionRange(placed.caret, placed.caret);
      }
    });
  };

  const wrap = (before: string, after: string, fallback: string) => {
    const el = area.current;
    const startAt = el?.selectionStart ?? text.length;
    const endAt = el?.selectionEnd ?? text.length;
    const selected = text.slice(startAt, endAt) || fallback;
    onChange({ text: text.slice(0, startAt) + before + selected + after + text.slice(endAt) });
    window.requestAnimationFrame(() => {
      if (el) {
        el.focus();
        el.setSelectionRange(startAt + before.length, startAt + before.length + selected.length);
      }
    });
  };

  const changeButton = (index: number, patch: Partial<ButtonDraft>) => {
    onChange({ buttons: buttons.map((b, i) => (i === index ? { ...b, ...patch } : b)) });
  };

  const pickFile = async (picked: File | undefined) => {
    if (!picked) {
      return;
    }
    if (picked.type !== 'image/png' && picked.type !== 'image/jpeg') {
      onNote({ text: 'Нужна картинка JPEG или PNG', tone: 'bad' });
      return;
    }
    if (picked.size > limits.image_bytes) {
      onNote({ text: 'Файл больше 5 МБ', tone: 'bad' });
      return;
    }
    setBusy('upload');
    onNote(null);
    try {
      onChange({ image: await upload<Image>(`${BASE}/images`, picked) });
    } catch (err) {
      onNote({ text: failText(err, 'Картинка не загрузилась'), tone: 'bad' });
    } finally {
      setBusy(null);
      if (file.current) {
        file.current.value = '';
      }
    }
  };

  const imageSrc = image ? imageUrl(image.id) : null;

  return (
    <>
      <section className="sec" aria-labelledby="bc-text-h">
        <div className="sec-head">
          <h2 id="bc-text-h" className="section-title">
            Сообщение
          </h2>
          <span className="muted">Текст с разметкой Telegram. Обычные эмодзи вставляются как есть.</span>
        </div>
        {extra}
        <div className="row">
          <span className="rl">Куда</span>
          <div className="rc">
            <span className="segs bc-channel" role="group" aria-label="Куда отправить">
              {CHANNELS.map((option) => (
                <button
                  key={option.key}
                  type="button"
                  aria-pressed={channel === option.key}
                  disabled={option.key !== 'bot' && !options.mail.ready && channel !== option.key}
                  onClick={() => {
                    onChange({ channel: option.key });
                    onNote(null);
                  }}
                >
                  {option.title}
                </button>
              ))}
            </span>
            {!options.mail.ready ? (
              <span className={`bc-channel-note ${mailOff ? 'warn-line' : 'cap'}`}>
                {options.mail.reason ?? 'Почта не отправляет'}.{' '}
                {options.mail.site ? <Link to="/admin/settings/general">Адрес сервиса</Link> : <Link to="/admin/settings/mail">Настройки почты</Link>}
              </span>
            ) : hasEmail(channel) ? (
              <span className="cap">Письмо соберётся из того же текста, картинки и кнопок. Внизу будет ссылка «Отписаться от рассылок».</span>
            ) : null}
          </div>
        </div>
        {hasEmail(channel) ? (
          <div className="row">
            <label className="rl" htmlFor="bc-subject">
              Тема письма
            </label>
            <div className="rc">
              <input
                id="bc-subject"
                className="field"
                value={subject}
                maxLength={limits.subject * 2}
                placeholder={subjectPlaceholder}
                aria-invalid={subjectProblem !== null}
                onChange={(e) => {
                  onChange({ subject: e.target.value });
                  onNote(null);
                }}
              />
              <span className="bc-count">
                <span className={subjectProblem ? 'err' : 'cap'}>{subjectProblem ?? ''}</span>
                <span className={`cap num ${subjectLength > limits.subject ? 'err' : ''}`}>
                  {count(subjectLength)} / {count(limits.subject)}
                </span>
              </span>
            </div>
          </div>
        ) : null}
        <div className="row">
          <div className="rc">
            <div className="bc-tools" role="toolbar" aria-label="Оформление текста">
              <button type="button" className="bc-tool hov" onClick={() => wrap('<b>', '</b>', 'жирный')} aria-label="Жирный">
                Ж
              </button>
              <button type="button" className="bc-tool hov" onClick={() => wrap('<i>', '</i>', 'курсив')} aria-label="Курсив">
                <em>К</em>
              </button>
              <button type="button" className="bc-tool hov" onClick={() => wrap('<a href="https://">', '</a>', 'текст ссылки')}>
                Ссылка
              </button>
              <button type="button" className="bc-tool hov" onClick={() => wrap('<tg-spoiler>', '</tg-spoiler>', 'скрытый текст')}>
                Скрытый текст
              </button>
              <button
                type="button"
                className="bc-tool hov"
                aria-expanded={emojiOpen}
                aria-controls="bc-emoji"
                onClick={() => {
                  setEmojiOpen(!emojiOpen);
                  setPasteNote(null);
                }}
              >
                Премиум-эмодзи
              </button>
            </div>
            {emojiOpen ? (
              <div id="bc-emoji" className="bc-emoji" role="group" aria-label="Премиум-эмодзи">
                <ol className="bc-steps">
                  <li>
                    Откройте {bot ? <a href={`https://t.me/${bot}`} target="_blank" rel="noreferrer noopener">@{bot}</a> : 'своего бота'} и отправьте{' '}
                    <code>/emoji</code> вместе с премиум-эмодзи, можно целой фразой.
                  </li>
                  <li>Скопируйте из ответа строку «Для текста».</li>
                  <li>Вставьте её сюда: эмодзи встанут в текст там, где стоял курсор, а если в текст ещё не нажимали — в конец.</li>
                </ol>
                <span className="bc-emoji-row">
                  <textarea
                    className="field bc-paste"
                    rows={2}
                    value={pasted}
                    placeholder="Вставьте ответ бота на /emoji"
                    aria-label="Ответ бота на /emoji"
                    onChange={(e) => {
                      setPasted(e.target.value);
                      setPasteNote(null);
                    }}
                    onPaste={(e) => {
                      const raw = e.clipboardData.getData('text');
                      if (raw) {
                        e.preventDefault();
                        pasteEmoji(raw);
                      }
                    }}
                  />
                  <button type="button" className="btn-line hov" onClick={() => pasteEmoji(pasted)} disabled={!pasted.trim()}>
                    Вставить
                  </button>
                </span>
                {pasteNote ? <span className={pasteNote.tone === 'bad' ? 'err' : 'cap'}>{pasteNote.text}</span> : null}
                <span className="cap">
                  Telegram покажет премиум-эмодзи, если у владельца бота есть Telegram Premium или бот купил дополнительные имена на Fragment. Иначе сообщение уйдёт с обычными
                  эмодзи. В системных уведомлениях и в письмах они тоже обычные.
                </span>
              </div>
            ) : null}
            <textarea
              ref={area}
              id="bc-text"
              className="field bc-area"
              rows={8}
              value={text}
              placeholder={placeholder}
              aria-label="Текст сообщения"
              aria-invalid={textProblem !== null}
              aria-describedby="bc-text-cap"
              onSelect={remember}
              onBlur={remember}
              onChange={(e) => {
                onChange({ text: e.target.value });
                onNote(null);
                caret.current = { start: e.target.selectionStart, end: e.target.selectionEnd };
              }}
              spellCheck
            />
            <span className="bc-count">
              <span id="bc-text-cap" className={textProblem ? 'err' : 'cap'}>
                {textProblem ?? (image ? 'С картинкой текст становится подписью к ней' : '')}
              </span>
              <span className={`cap num ${length > limit ? 'err' : ''}`}>
                {count(length)} / {count(limit)}
              </span>
            </span>
            <details className="bc-hint">
              <summary>Как оформить текст</summary>
              <ul>
                <li>
                  <code>{'<b>жирный</b>'}</code>, <code>{'<i>курсив</i>'}</code>, <code>{'<u>подчёркнутый</u>'}</code>, <code>{'<s>зачёркнутый</s>'}</code>
                </li>
                <li>
                  <code>{'<a href="https://example.com">ссылка</a>'}</code>, <code>{'<tg-spoiler>скрытый</tg-spoiler>'}</code>,{' '}
                  <code>{'<blockquote>цитата</blockquote>'}</code>, <code>{'<code>код</code>'}</code>
                </li>
                <li>
                  Премиум-эмодзи — кнопка «Премиум-эмодзи» над полем: отправьте боту <code>/emoji</code> с нужными эмодзи и вставьте строку из ответа. Строка выглядит так:{' '}
                  <code>{'<tg-emoji emoji-id="5368324170671202286">🔥</tg-emoji>'}</code>.
                </li>
                <li>
                  Премиум-эмодзи в тексте и значок на кнопке Telegram показывает, если у владельца бота есть Telegram Premium или бот купил дополнительные имена на Fragment.
                  Если Telegram их не примет, сообщение уйдёт с обычными эмодзи и без значков. Там, где премиум-эмодзи показать нельзя (например, в системных уведомлениях), и
                  в сообщении, которое переслал человек без Premium, видно обычное эмодзи.
                </li>
                <li>Цвет кнопки — синяя, зелёная или красная; без цвета кнопка выглядит так, как принято в приложении клиента.</li>
                <li>В письме премиум-эмодзи становятся обычными, а у кнопок нет цвета и значка.</li>
              </ul>
            </details>
            <div className="bc-inline">{preview}</div>
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="bc-image-h">
        <div className="sec-head">
          <h2 id="bc-image-h" className="section-title">
            Картинка
          </h2>
          <span className="muted">Необязательно. JPEG или PNG до 5 МБ, текст к ней — до {count(limits.caption)} знаков.</span>
        </div>
        <div className="row">
          <div className="rc">
            <input
              ref={file}
              className="sr-only"
              type="file"
              accept="image/png,image/jpeg"
              id="bc-file"
              onChange={(e) => void pickFile(e.target.files?.[0])}
              tabIndex={-1}
            />
            {image && imageSrc ? (
              <div className="bc-image">
                <img src={imageSrc} alt="Картинка сообщения" className="bc-thumb" />
                <span className="bc-image-info">
                  <span>
                    {image.width} × {image.height}
                  </span>
                  <span className="cap">
                    {image.content_type === 'image/png' ? 'PNG' : 'JPEG'}, {fileSize(image.size)}
                  </span>
                </span>
                <button type="button" className="btn-line hov" onClick={() => file.current?.click()} disabled={busy !== null}>
                  Заменить
                </button>
                <button type="button" className="bc-link bad" onClick={() => onChange({ image: null })} disabled={busy !== null}>
                  Убрать
                </button>
              </div>
            ) : (
              <span>
                <button type="button" className="btn-line hov bc-upload" onClick={() => file.current?.click()} disabled={busy !== null}>
                  <Icon name="image" size={18} />
                  &nbsp;{busy === 'upload' ? 'Загружаем…' : 'Загрузить картинку'}
                </button>
              </span>
            )}
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="bc-buttons-h">
        <div className="sec-head">
          <h2 id="bc-buttons-h" className="section-title">
            Кнопки
          </h2>
          <span className="muted">
            Встают под сообщением, каждая в своей строке. Раздел бота открывается отдельным сообщением, само сообщение остаётся в чате. Цвет и значок из премиум-эмодзи
            необязательны{hasEmail(channel) ? '; в письме кнопка будет без них' : ''}.
          </span>
        </div>
        <div className="row">
          <div className="rc bc-buttons">
            {buttons.map((button, index) => {
              const problem = buttonProblems[index];
              const sectionTitle = options.sections.find((s) => s.key === button.section);
              return (
                <div key={index} className="bc-bwrap">
                  <div className="bc-button">
                    <input
                      className="field"
                      value={button.text}
                      maxLength={limits.button_text * 2}
                      placeholder="Надпись на кнопке"
                      aria-label={`Надпись кнопки ${index + 1}`}
                      aria-invalid={tried && problem !== null && !button.text.trim()}
                      onChange={(e) => changeButton(index, { text: e.target.value })}
                    />
                    <span className="segs" role="group" aria-label={`Что открывает кнопка ${index + 1}`}>
                      <button type="button" aria-pressed={button.kind === 'url'} onClick={() => changeButton(index, { kind: 'url' })}>
                        Ссылка
                      </button>
                      <button type="button" aria-pressed={button.kind === 'section'} onClick={() => changeButton(index, { kind: 'section' })}>
                        Раздел бота
                      </button>
                    </span>
                    {button.kind === 'url' ? (
                      <input
                        className="field mono bc-target"
                        value={button.url}
                        inputMode="url"
                        placeholder="https://t.me/ваш_канал"
                        aria-label={`Адрес кнопки ${index + 1}`}
                        onChange={(e) => changeButton(index, { url: e.target.value })}
                      />
                    ) : (
                      <select
                        className="field bc-target"
                        value={button.section}
                        aria-label={`Раздел бота для кнопки ${index + 1}`}
                        onChange={(e) => changeButton(index, { section: e.target.value as Section })}
                      >
                        {options.sections.map((section) => (
                          <option key={section.key} value={section.key}>
                            {section.available ? section.title : `${section.title} — модуль выключен`}
                          </option>
                        ))}
                      </select>
                    )}
                    <button
                      type="button"
                      className="bc-remove hov"
                      aria-label={`Убрать кнопку ${index + 1}`}
                      onClick={() => onChange({ buttons: buttons.filter((_, i) => i !== index) })}
                    >
                      <Icon name="close" size={18} />
                    </button>
                  </div>
                  <div className="bc-blook">
                    <span className="segs" role="group" aria-label={`Цвет кнопки ${index + 1}`}>
                      {BUTTON_STYLES.map((style) => (
                        <button
                          key={style.value || 'plain'}
                          type="button"
                          aria-pressed={button.style === style.value}
                          onClick={() => changeButton(index, { style: style.value })}
                        >
                          {style.label}
                        </button>
                      ))}
                    </span>
                    <input
                      className="field mono bc-emoji-id"
                      value={button.emoji_id}
                      inputMode="numeric"
                      placeholder="Премиум-эмодзи: код"
                      aria-label={`Код премиум-эмодзи для значка кнопки ${index + 1}`}
                      aria-invalid={emojiIdProblem(button.emoji_id) !== null}
                      onChange={(e) => changeButton(index, { emoji_id: e.target.value })}
                    />
                  </div>
                  {problem && (tried || button.text.trim() || button.url.trim() || button.emoji_id.trim()) ? <span className="err">{problem}</span> : null}
                  {button.kind === 'section' && sectionTitle && !sectionTitle.available ? (
                    <span className="warn-line">Модуль раздела выключен: кнопка откроет главное меню</span>
                  ) : null}
                </div>
              );
            })}
            {buttons.length < limits.buttons ? (
              <button
                type="button"
                className="btn-line hov bc-add"
                onClick={() => onChange({ buttons: [...buttons, { text: '', kind: 'url', url: '', section: 'menu', style: '', emoji_id: '' }] })}
              >
                Добавить кнопку
              </button>
            ) : (
              <span className="cap">Больше {limits.buttons} кнопок Telegram покажет неудобно</span>
            )}
          </div>
        </div>
      </section>
    </>
  );
}

type TestProps = {
  options: Options;
  value: MessageValue;
  check: MessageCheck;
  busy: string | null;
  setBusy: (busy: string | null) => void;
  onTried: () => void;
  lead: string;
};

export function TestFields({ options, value, check, busy, setBusy, onTried, lead }: TestProps) {
  const { channel } = value;
  const [query, setQuery] = useState('');
  const [found, setFound] = useState<ClientRow[] | null>(null);
  const [findError, setFindError] = useState<string | null>(null);
  const [target, setTarget] = useState<Target | null>(storedTarget);
  const [testNote, setTestNote] = useState<Note | null>(null);
  const [mailTo, setMailTo] = useState(() => stored(MAIL_KEY));
  const [mailNote, setMailNote] = useState<Note | null>(null);

  useEffect(() => {
    const needle = query.trim();
    if (!needle) {
      setFound(null);
      setFindError(null);
      return;
    }
    const timer = window.setTimeout(() => {
      api<{ items: ClientRow[] }>(`/api/admin/clients?limit=6&q=${encodeURIComponent(needle)}`)
        .then((page) => {
          setFound(page.items);
          setFindError(null);
        })
        .catch((err) => setFindError(failText(err, 'Поиск не сработал')));
    }, 300);
    return () => window.clearTimeout(timer);
  }, [query]);

  const payload = { text: value.text, image_id: value.image?.id ?? null, buttons: value.buttons.map(toSpec) };

  const test = async () => {
    if (!target) {
      return;
    }
    onTried();
    setBusy('test');
    setTestNote(null);
    try {
      const report = await api<{ result: 'sent' | 'blocked'; message: string }>(`${BASE}/test`, {
        method: 'POST',
        body: { client_id: target.id, ...payload },
      });
      setTestNote(report.result === 'sent' ? { text: `Проверка отправлена: ${target.name}`, tone: 'ok' } : { text: report.message, tone: 'bad' });
    } catch (err) {
      setTestNote({ text: failText(err, 'Проверка не отправилась'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const testMail = async () => {
    const address = mailTo.trim();
    if (!address) {
      return;
    }
    onTried();
    setBusy('mail');
    setMailNote(null);
    keep(MAIL_KEY, address);
    try {
      const report = await api<{ result: 'sent' | 'blocked'; message: string }>(`${BASE}/test`, {
        method: 'POST',
        body: { email: address, subject: value.subject, ...payload },
      });
      setMailNote({ text: report.message, tone: 'ok' });
    } catch (err) {
      setMailNote({ text: failText(err, 'Письмо не отправилось'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const choose = (row: ClientRow) => {
    const picked = { id: row.client.id, name: displayName(row), handle: handleOf(row) };
    setTarget(picked);
    storeTarget(picked);
    setQuery('');
    setFound(null);
    setTestNote(null);
  };

  return (
    <section className="sec" aria-labelledby="bc-test-h">
      <div className="sec-head">
        <h2 id="bc-test-h" className="section-title">
          Проверка
        </h2>
        <span className="muted">{lead}</span>
      </div>
      {hasBot(channel) ? (
        <div className="row">
          <span className="rl">В бот, клиенту</span>
          <div className="rc">
            {target ? (
              <span className="bc-picked">
                <strong>{target.name}</strong>
                {target.handle ? <span className="muted">{target.handle}</span> : null}
                <button
                  type="button"
                  className="bc-link"
                  onClick={() => {
                    setTarget(null);
                    storeTarget(null);
                    setTestNote(null);
                  }}
                >
                  Сменить
                </button>
              </span>
            ) : (
              <>
                <input
                  className="field"
                  type="search"
                  value={query}
                  maxLength={200}
                  placeholder="Имя, @username, Telegram ID или email"
                  aria-label="Найти клиента для проверки"
                  onChange={(e) => setQuery(e.target.value)}
                />
                {findError ? <span className="err">{findError}</span> : null}
                {found && found.length === 0 ? <span className="cap">Никого не нашли</span> : null}
                {found && found.length > 0 ? (
                  <div className="bc-found">
                    {found.map((row) => (
                      <button key={row.client.id} type="button" className="hov" onClick={() => choose(row)} disabled={!row.telegram_id}>
                        <span>{displayName(row)}</span>
                        <span className="muted">{row.telegram_id ? handleOf(row) : 'нет Telegram'}</span>
                      </button>
                    ))}
                  </div>
                ) : null}
              </>
            )}
            <span>
              <button type="button" className="btn-line hov" onClick={() => void test()} disabled={!target || !check.messageOk || busy !== null}>
                {busy === 'test' ? 'Отправляем…' : 'Отправить в бот'}
              </button>
            </span>
            {testNote ? (
              <span role="status" className={`bc-result ${testNote.tone === 'plain' ? '' : testNote.tone}`}>
                {testNote.text}
              </span>
            ) : null}
            {!options.bot ? <span className="warn-line">Бот не подключён: проверка не уйдёт, пока не задан токен</span> : null}
          </div>
        </div>
      ) : null}
      {hasEmail(channel) ? (
        <div className="row">
          <label className="rl" htmlFor="bc-mail-to">
            На почту, на адрес
          </label>
          <div className="rc">
            <input
              id="bc-mail-to"
              className="field"
              type="email"
              inputMode="email"
              value={mailTo}
              maxLength={254}
              placeholder="you@example.com"
              onChange={(e) => {
                setMailTo(e.target.value);
                setMailNote(null);
              }}
            />
            <span>
              <button type="button" className="btn-line hov" onClick={() => void testMail()} disabled={!mailTo.trim() || !check.contentOk || check.mailOff || busy !== null}>
                {busy === 'mail' ? 'Отправляем…' : 'Отправить письмо'}
              </button>
            </span>
            {mailNote ? (
              <span role="status" className={`bc-result ${mailNote.tone === 'plain' ? '' : mailNote.tone}`}>
                {mailNote.text}
              </span>
            ) : null}
          </div>
        </div>
      ) : null}
    </section>
  );
}
