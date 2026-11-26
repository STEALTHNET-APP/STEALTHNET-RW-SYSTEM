import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { dateLong } from '../../lib/format';
import { useSession } from '../session';
import { Switch } from '../ui';
import '../form.css';
import { BotTabs } from './BotTabs';
import { SettingsLayout } from './SettingsLayout';
import { TelegramText, unclosedTag, visibleLength } from './telegram';

type Kind = 'text' | 'button';
type Style = 'primary' | 'success' | 'danger';
type ButtonOverride = { text?: string; emoji_id?: string; style?: Style; hidden?: boolean };
type Var = { name: string; hint: string };
type Entry = {
  key: string;
  section: string;
  kind: Kind;
  label: string;
  default: string;
  vars: Var[];
  optional: boolean;
  text: string | null;
  button: ButtonOverride | null;
  default_en: string;
  text_en: string | null;
};
type Lang = 'ru' | 'en';
type SectionInfo = { key: string; title: string; image: string | null };
type View = {
  sections: SectionInfo[];
  entries: Entry[];
  english: boolean;
  menu_columns: number;
  emoji_rejected_at: string | null;
  text_max: number;
  caption_max: number;
  button_max: number;
};
type Slot = { slot: string; mode: 'common' | 'own' | 'off' | null; version: string | null };
type Images = { slots: Slot[] };
type ButtonDraft = { text: string; emoji_id: string; style: Style | ''; hidden: boolean };

const STYLES: Array<{ value: Style | ''; label: string }> = [
  { value: '', label: 'Обычная' },
  { value: 'primary', label: 'Синяя' },
  { value: 'success', label: 'Зелёная' },
  { value: 'danger', label: 'Красная' },
];

const SAMPLES: Record<string, string> = {
  name: 'Алексей',
  tariff: 'Стандарт',
  current: 'Базовый',
  status: 'активна',
  date: '14 февраля 2027',
  days: '90 дней',
  used: '12 ГБ',
  limit: '100 ГБ',
  reset: ', обновится 1 декабря',
  count: '2',
  url: 'https://sub.example.com/s/9f3c4a',
  error: 'Сервис VPN сейчас недоступен.',
  title: 'iPhone 15',
  n: '1',
  details: 'iOS 18 · Happ',
  amount: '500 ₽',
  price: '299 ₽',
  per_month: '249 ₽',
  savings: '17',
  category: 'Обычные тарифы',
  included: '3 устройства',
  label: 'Стандарт · 3 месяца',
  total: '747 ₽',
  stars: '500 звёзд',
  method: 'Картой',
  hint: 'Visa, Mastercard, Мир',
  missing: '200 ₽',
  balance: '100 ₽',
  what: 'Стандарт.',
  example: 'например 500',
  id: '42',
  code: 'Промокод WELCOME',
  percent: '10',
  link: 'https://t.me/your_bot?start=ref_ABC',
  minimum: ' Минимум — 500 ₽.',
  extra: '1',
  can_add: '2',
  variant: 'Европа',
  months: '3',
  who: 'Алексей (@morozov_a, Telegram 5093118422)',
  ip: '203.0.113.7',
  agent: 'Chrome на macOS',
  body: 'Отправили чек повторно на почту.',
  comment: 'реквизиты не подходят',
};

const SAMPLES_EN: Record<string, string> = {
  ...SAMPLES,
  name: 'Ann',
  tariff: 'Standard',
  current: 'Basic',
  status: 'active',
  date: '14 February 2027',
  days: '90 days',
  used: '12 GB',
  limit: '100 GB',
  reset: ', resets 1 December',
  error: 'The VPN service is unavailable right now.',
  details: 'iOS 18 · Happ',
  amount: '$5.00',
  price: '$2.99',
  per_month: '$2.49',
  category: 'Regular plans',
  included: '3 devices',
  label: 'Standard · 90 days',
  total: '$7.47',
  stars: '500 stars',
  method: 'Bank card',
  hint: 'Visa, Mastercard',
  missing: '$2.00',
  balance: '$1.00',
  what: 'Standard.',
  example: 'for example 10 or 12.50',
  code: 'Promo code WELCOME',
  minimum: ' Minimum: $5.00.',
  who: 'Ann (@ann_w, Telegram 5093118422)',
  agent: 'Chrome on macOS',
  body: 'We have sent the receipt to your email again.',
  comment: 'the details are wrong',
  variant: 'Europe',
};

function englishEntry(entry: Entry): Entry {
  return { ...entry, default: entry.default_en, text: entry.text_en };
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function fill(template: string, vars: Var[], brand: string, html: boolean, lang: Lang = 'ru'): string {
  const samples = lang === 'en' ? SAMPLES_EN : SAMPLES;
  return template.replace(/\{([a-z0-9_]{1,32})\}/g, (whole, name: string) => {
    if (!vars.some((v) => v.name === name)) {
      return whole;
    }
    const value = name === 'brand' ? brand : (samples[name] ?? name);
    return html ? escapeHtml(value) : value;
  });
}

function unknownVar(value: string, vars: Var[]): string | null {
  const found = value.match(/\{([a-z0-9_]{1,32})\}/g) ?? [];
  for (const raw of found) {
    const name = raw.slice(1, -1);
    if (!vars.some((v) => v.name === name)) {
      return vars.length
        ? `подстановки ${raw} здесь нет, можно: ${vars.map((v) => `{${v.name}}`).join(', ')}`
        : `подстановки ${raw} здесь нет`;
    }
  }
  return null;
}

function buttonDraft(entry: Entry): ButtonDraft {
  return {
    text: entry.button?.text ?? '',
    emoji_id: entry.button?.emoji_id ?? '',
    style: entry.button?.style ?? '',
    hidden: entry.button?.hidden ?? false,
  };
}

function sameButton(a: ButtonDraft, b: ButtonDraft): boolean {
  return a.text.trim() === b.text.trim() && a.emoji_id.trim() === b.emoji_id.trim() && a.style === b.style && a.hidden === b.hidden;
}

function draftIsPlain(entry: Entry, draft: ButtonDraft): boolean {
  const text = draft.text.trim();
  return (text === '' || text === entry.default) && draft.emoji_id.trim() === '' && draft.style === '' && !draft.hidden;
}

function textProblem(entry: Entry, value: string, max: number): string | null {
  if (!value.trim()) {
    return entry.optional ? null : 'текст не может быть пустым';
  }
  if (value.length > max) {
    return `длиннее ${max} символов`;
  }
  return unknownVar(value, entry.vars) ?? unclosedTag(value);
}

function labelProblem(entry: Entry, label: string, max: number): string | null {
  const text = label.trim();
  if ([...text].length > max) {
    return `надпись длиннее ${max} символов`;
  }
  if (text.includes('\n')) {
    return 'надпись на кнопке должна быть в одну строку';
  }
  return text ? unknownVar(text, entry.vars) : null;
}

function buttonProblem(entry: Entry, draft: ButtonDraft, max: number): string | null {
  const text = draft.text.trim();
  if ([...text].length > max) {
    return `надпись длиннее ${max} символов`;
  }
  if (text && unknownVar(text, entry.vars)) {
    return unknownVar(text, entry.vars);
  }
  if (draft.emoji_id.trim() && !/^\d{1,32}$/.test(draft.emoji_id.trim())) {
    return 'код премиум-эмодзи — только цифры, его присылает бот по команде /emoji';
  }
  return null;
}

export function SettingsBotTexts() {
  const { me } = useSession();
  const [view, setView] = useState<View | null>(null);
  const [images, setImages] = useState<Images | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [texts, setTexts] = useState<Record<string, string>>({});
  const [buttons, setButtons] = useState<Record<string, ButtonDraft>>({});
  const [textsEn, setTextsEn] = useState<Record<string, string>>({});
  const [labelsEn, setLabelsEn] = useState<Record<string, string>>({});
  const [lang, setLang] = useState<Lang>('ru');
  const [section, setSection] = useState('menu');
  const [query, setQuery] = useState('');
  const [active, setActive] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const areas = useRef<Record<string, HTMLTextAreaElement | null>>({});

  const apply = useCallback((data: View) => {
    setView(data);
    const nextTexts: Record<string, string> = {};
    const nextButtons: Record<string, ButtonDraft> = {};
    const nextTextsEn: Record<string, string> = {};
    const nextLabelsEn: Record<string, string> = {};
    for (const entry of data.entries) {
      if (entry.kind === 'text') {
        nextTexts[entry.key] = entry.text ?? entry.default;
        nextTextsEn[entry.key] = entry.text_en ?? entry.default_en;
      } else {
        nextButtons[entry.key] = buttonDraft(entry);
        nextLabelsEn[entry.key] = entry.text_en ?? '';
      }
    }
    setTexts(nextTexts);
    setButtons(nextButtons);
    setTextsEn(nextTextsEn);
    setLabelsEn(nextLabelsEn);
  }, []);

  const load = useCallback(async () => {
    try {
      const [data, pictures] = await Promise.all([api<View>('/api/admin/bot/content'), api<Images>('/api/admin/bot/images')]);
      apply(data);
      setImages(pictures);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Тексты бота не загрузились'));
    }
  }, [apply]);

  useEffect(() => {
    void load();
  }, [load]);

  const changedRu = useMemo(() => {
    if (!view) {
      return [] as Entry[];
    }
    return view.entries.filter((entry) =>
      entry.kind === 'text'
        ? (texts[entry.key] ?? '') !== (entry.text ?? entry.default)
        : !sameButton(buttons[entry.key] ?? buttonDraft(entry), buttonDraft(entry)),
    );
  }, [view, texts, buttons]);

  const changedEn = useMemo(() => {
    if (!view) {
      return [] as Entry[];
    }
    return view.entries.filter((entry) =>
      entry.kind === 'text'
        ? (textsEn[entry.key] ?? '') !== (entry.text_en ?? entry.default_en)
        : (labelsEn[entry.key] ?? '').trim() !== (entry.text_en ?? '').trim(),
    );
  }, [view, textsEn, labelsEn]);

  const changed = lang === 'en' ? changedEn : changedRu;

  const imageFor = useCallback(
    (sectionKey: string): string | null => {
      const slotName = view?.sections.find((s) => s.key === sectionKey)?.image;
      if (!slotName || !images) {
        return null;
      }
      const own = images.slots.find((s) => s.slot === slotName);
      const common = images.slots.find((s) => s.slot === 'common');
      if (!own || own.mode === 'off') {
        return null;
      }
      if (own.mode === 'own' && own.version) {
        return `/api/admin/bot/images/${own.slot}/file?v=${own.version}`;
      }
      return common?.version ? `/api/admin/bot/images/common/file?v=${common.version}` : null;
    },
    [view, images],
  );

  if (!view) {
    return (
      <SettingsLayout>
        <BotTabs />
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем тексты бота…</p>
        )}
      </SettingsLayout>
    );
  }

  const unsaved = changedRu.length + changedEn.length;
  const dirty = unsaved > 0;
  const english = lang === 'en';
  const needle = query.trim().toLowerCase();
  const visible = view.entries.filter((entry) => {
    if (needle) {
      const value = english
        ? entry.kind === 'text'
          ? textsEn[entry.key]
          : labelsEn[entry.key]
        : entry.kind === 'text'
          ? texts[entry.key]
          : buttons[entry.key]?.text;
      return [entry.label, english ? entry.default_en : entry.default, value ?? ''].some((part) => part.toLowerCase().includes(needle));
    }
    return entry.section === section;
  });
  const groups = view.sections
    .map((info) => ({ info, entries: visible.filter((e) => e.section === info.key) }))
    .filter((group) => group.entries.length > 0);
  const counts = new Map<string, number>();
  for (const entry of changed) {
    counts.set(entry.section, (counts.get(entry.section) ?? 0) + 1);
  }

  const setText = (key: string, value: string) => {
    if (english) {
      setTextsEn({ ...textsEn, [key]: value });
    } else {
      setTexts({ ...texts, [key]: value });
    }
  };

  const insert = (entry: Entry, name: string) => {
    const token = `{${name}}`;
    const area = areas.current[entry.key];
    const current = (english ? textsEn[entry.key] : texts[entry.key]) ?? '';
    if (!area) {
      setText(entry.key, current + token);
      return;
    }
    const start = area.selectionStart ?? current.length;
    const end = area.selectionEnd ?? current.length;
    const next = current.slice(0, start) + token + current.slice(end);
    setText(entry.key, next);
    window.requestAnimationFrame(() => {
      area.focus();
      area.setSelectionRange(start + token.length, start + token.length);
    });
  };

  const save = async () => {
    if (!dirty || busy) {
      return;
    }
    for (const entry of changedRu) {
      const problem =
        entry.kind === 'text'
          ? textProblem(entry, texts[entry.key] ?? '', view.text_max)
          : buttonProblem(entry, buttons[entry.key] ?? buttonDraft(entry), view.button_max);
      if (problem) {
        setStatus({ text: `«${entry.label}»: ${problem}`, bad: true });
        setLang('ru');
        setActive(entry.key);
        setSection(entry.section);
        return;
      }
    }
    for (const entry of changedEn) {
      const problem =
        entry.kind === 'text'
          ? textProblem(englishEntry(entry), textsEn[entry.key] ?? '', view.text_max)
          : labelProblem(entry, labelsEn[entry.key] ?? '', view.button_max);
      if (problem) {
        setStatus({ text: `«${entry.label}», английский: ${problem}`, bad: true });
        setLang('en');
        setActive(entry.key);
        setSection(entry.section);
        return;
      }
    }
    const body: {
      texts: Record<string, string | null>;
      buttons: Record<string, ButtonOverride | null>;
      en?: { texts: Record<string, string | null>; buttons: Record<string, string | null> };
    } = {
      texts: {},
      buttons: {},
    };
    if (changedEn.length) {
      const en = { texts: {} as Record<string, string | null>, buttons: {} as Record<string, string | null> };
      for (const entry of changedEn) {
        if (entry.kind === 'text') {
          const value = textsEn[entry.key] ?? '';
          en.texts[entry.key] = value === entry.default_en ? null : value;
        } else {
          const label = (labelsEn[entry.key] ?? '').trim();
          en.buttons[entry.key] = label && label !== entry.default_en ? label : null;
        }
      }
      body.en = en;
    }
    for (const entry of changedRu) {
      if (entry.kind === 'text') {
        const value = texts[entry.key] ?? '';
        body.texts[entry.key] = value === entry.default ? null : value;
      } else {
        const draft = buttons[entry.key] ?? buttonDraft(entry);
        body.buttons[entry.key] = draftIsPlain(entry, draft)
          ? null
          : {
              text: draft.text.trim() || undefined,
              emoji_id: draft.emoji_id.trim() || undefined,
              style: draft.style || undefined,
              hidden: draft.hidden || undefined,
            };
      }
    }
    setBusy(true);
    setStatus(null);
    try {
      apply(await api<View>('/api/admin/bot/content', { method: 'PUT', body }));
      setStatus({ text: 'Сохранено: бот уже пишет по-новому', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Тексты не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const reset = () => {
    apply(view);
    setStatus(null);
  };

  return (
    <SettingsLayout>
      <BotTabs />
      <section className="sec" aria-labelledby="bt-h">
        <div className="sec-head">
          <h2 id="bt-h" className="section-title">
            Тексты и кнопки бота
          </h2>
          <span className="muted">
            Здесь меняется всё, что пишет бот: экраны, уведомления и надписи на кнопках. Обычные эмодзи вставляются в текст как есть.
          </span>
        </div>
        {view.emoji_rejected_at ? (
          <div className="row">
            <p className="bt-warn" role="status">
              {dateLong(view.emoji_rejected_at)} Telegram не принял премиум-эмодзи, и бот отправил сообщения с обычными. Премиум-эмодзи показываются, только если у владельца бота
              есть Telegram Premium или бот купил дополнительные имена на Fragment.
            </p>
          </div>
        ) : null}
        <details className="row bt-help">
          <summary>Как оформить текст и вставить премиум-эмодзи</summary>
          <div className="bt-help-body">
            <p>Разметка Telegram прямо в тексте:</p>
            <ul>
              <li>
                <code>{'<b>жирный</b>'}</code>, <code>{'<i>курсив</i>'}</code>, <code>{'<u>подчёркнутый</u>'}</code>, <code>{'<s>зачёркнутый</s>'}</code>
              </li>
              <li>
                <code>{'<code>моноширинный</code>'}</code>, <code>{'<tg-spoiler>скрытый</tg-spoiler>'}</code>, <code>{'<blockquote>цитата</blockquote>'}</code>
              </li>
              <li>
                <code>{'<a href="https://example.com">ссылка</a>'}</code>
              </li>
            </ul>
            <p>
              Премиум-эмодзи: отправьте боту команду <code>/emoji</code> вместе с нужными эмодзи в одном сообщении, можно целой фразой. Бот ответит строкой для текста вида{' '}
              <code>{'<tg-emoji emoji-id="5368324170671202286">🔥</tg-emoji>'}</code> — её вставляют в текст, и кодами — их вставляют в поле «Код премиум-эмодзи» у
              кнопки. Работает, если у владельца бота есть Telegram Premium или бот купил дополнительные имена на Fragment; иначе бот покажет обычное эмодзи.
            </p>
            <p>Слова в фигурных скобках, например {'{tariff}'}, бот заменит на настоящие значения. Пустой текст у необязательной строки — строка не показывается.</p>
          </div>
        </details>
      </section>

      <div className="bt-toolbar">
        <span className="segs bt-langs" role="group" aria-label="Язык текстов">
          {(['ru', 'en'] as Lang[]).map((value) => (
            <button key={value} type="button" aria-pressed={lang === value} onClick={() => setLang(value)}>
              {value === 'ru' ? 'Русский' : 'English'}
              {(value === 'ru' ? changedRu.length : changedEn.length) ? <span className="bt-count">{value === 'ru' ? changedRu.length : changedEn.length}</span> : null}
            </button>
          ))}
        </span>
        <input
          className="field bt-search"
          type="search"
          placeholder="Найти текст или кнопку"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label="Поиск по текстам бота"
        />
        {needle ? null : (
          <div className="bt-sections" role="tablist" aria-label="Разделы бота">
            {view.sections.map((info) => (
              <button
                key={info.key}
                type="button"
                role="tab"
                aria-selected={section === info.key}
                className="bt-section hov"
                onClick={() => setSection(info.key)}
              >
                {info.title}
                {counts.get(info.key) ? <span className="bt-count">{counts.get(info.key)}</span> : null}
              </button>
            ))}
          </div>
        )}
      </div>

      {english ? (
        <p className="cap bt-lang-note">
          {view.english
            ? 'Эти тексты видят клиенты, которые выбрали английский. Пустое поле — стандартный перевод. Цвет, премиум-эмодзи и показ кнопок общие для обоих языков.'
            : 'Английский сейчас выключен в настройках «Языки»: тексты сохранятся и заработают, когда его включат.'}
        </p>
      ) : null}
      {groups.length === 0 ? <p className="muted">Ничего не нашлось</p> : null}
      {groups.map(({ info, entries }) => {
        const image = imageFor(info.key);
        return (
          <section key={info.key} className="sec" aria-labelledby={`bt-s-${info.key}`}>
            <div className="sec-head bt-head">
              <h3 id={`bt-s-${info.key}`} className="section-title">
                {info.title}
              </h3>
              {info.key === 'menu' || info.key === 'subscription' ? (
                <Link to="/admin/settings/bot/layout" className="bt-reset hov bt-order">
                  Порядок кнопок
                </Link>
              ) : null}
              {image ? <span className="cap">Сообщения раздела уходят с картинкой: текст до {view.caption_max} знаков, длиннее — без картинки.</span> : null}
            </div>
            {entries.map((entry) =>
              entry.kind === 'text' ? (
                <TextRow
                  key={entry.key}
                  entry={english ? englishEntry(entry) : entry}
                  value={(english ? textsEn[entry.key] : texts[entry.key]) ?? ''}
                  max={image ? view.caption_max : view.text_max}
                  hardMax={view.text_max}
                  image={image}
                  brand={me.brand_name}
                  lang={lang}
                  untranslated={english && entry.text !== null && entry.text_en === null}
                  open={active === entry.key}
                  onFocus={() => setActive(entry.key)}
                  onChange={(value) => {
                    setText(entry.key, value);
                    setStatus(null);
                  }}
                  onInsert={(name) => insert(entry, name)}
                  areaRef={(el) => {
                    areas.current[entry.key] = el;
                  }}
                  disabled={busy}
                />
              ) : english ? (
                <LabelRow
                  key={entry.key}
                  entry={entry}
                  label={labelsEn[entry.key] ?? ''}
                  draft={buttons[entry.key] ?? buttonDraft(entry)}
                  max={view.button_max}
                  brand={me.brand_name}
                  onChange={(label) => {
                    setLabelsEn({ ...labelsEn, [entry.key]: label });
                    setStatus(null);
                  }}
                  disabled={busy}
                />
              ) : (
                <ButtonRow
                  key={entry.key}
                  entry={entry}
                  draft={buttons[entry.key] ?? buttonDraft(entry)}
                  max={view.button_max}
                  brand={me.brand_name}
                  onChange={(draft) => {
                    setButtons({ ...buttons, [entry.key]: draft });
                    setStatus(null);
                  }}
                  disabled={busy}
                />
              ),
            )}
          </section>
        );
      })}

      <div className="save-row bt-save">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || !dirty}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" disabled={busy || !dirty} onClick={reset}>
          Отменить
        </button>
        <span role="status" className={status?.bad ? 'bad' : undefined}>
          {status?.text ?? (dirty ? `Не сохранено: ${unsaved}` : '')}
        </span>
      </div>
    </SettingsLayout>
  );
}

type TextRowProps = {
  entry: Entry;
  value: string;
  max: number;
  hardMax: number;
  image: string | null;
  brand: string;
  lang: Lang;
  untranslated: boolean;
  open: boolean;
  onFocus: () => void;
  onChange: (value: string) => void;
  onInsert: (name: string) => void;
  areaRef: (el: HTMLTextAreaElement | null) => void;
  disabled: boolean;
};

function TextRow({ entry, value, max, hardMax, image, brand, lang, untranslated, open, onFocus, onChange, onInsert, areaRef, disabled }: TextRowProps) {
  const id = `bt-${entry.key}-${lang}`;
  const problem = textProblem(entry, value, hardMax);
  const filled = fill(value, entry.vars, brand, true, lang);
  const length = visibleLength(filled);
  const over = image !== null && length > max;
  const custom = entry.text !== null || value !== entry.default;
  const rows = Math.min(8, Math.max(2, value.split('\n').length + (value.length > 90 ? 1 : 0)));
  return (
    <div className="row bt-entry">
      <label htmlFor={id} className="rl">
        {entry.label}
        {custom ? <span className="bt-mark">изменено</span> : null}
        {untranslated ? <span className="bt-mark">русский текст свой</span> : null}
      </label>
      <div className="rc">
        <textarea
          id={id}
          ref={areaRef}
          className="field bt-text"
          rows={rows}
          value={value}
          placeholder={entry.optional ? 'Пусто — строка не показывается' : entry.default}
          aria-invalid={problem !== null}
          aria-describedby={`${id}-cap`}
          onFocus={onFocus}
          onChange={(e) => onChange(e.target.value)}
          disabled={disabled}
          spellCheck
          lang={lang}
        />
        {entry.vars.length ? (
          <span className="bt-vars" aria-label="Подстановки">
            {entry.vars.map((v) => (
              <button key={v.name} type="button" className="bt-chip hov" title={v.hint} onClick={() => onInsert(v.name)} disabled={disabled}>
                {`{${v.name}}`}
                <span className="bt-chip-hint">{v.hint}</span>
              </button>
            ))}
          </span>
        ) : null}
        <span className="bt-meta">
          <span id={`${id}-cap`} className={problem || over ? 'err' : 'cap'}>
            {problem ??
              (over
                ? `С картинкой Telegram показывает до ${max} знаков, это сообщение уйдёт без картинки`
                : entry.optional
                  ? 'Необязательная строка'
                  : '')}
          </span>
          <span className="cap num">
            {length} / {image ? max : hardMax}
          </span>
          {value !== entry.default ? (
            <button type="button" className="bt-reset hov" onClick={() => onChange(entry.default)} disabled={disabled}>
              Как было
            </button>
          ) : null}
        </span>
        {open || custom ? (
          <div className="bt-preview" aria-label="Как увидит клиент">
            {image ? <img src={image} alt="" className="bt-preview-img" /> : null}
            {filled.trim() ? (
              <div className="bt-bubble">
                <TelegramText html={filled} />
              </div>
            ) : (
              <span className="cap">Строка не будет показана</span>
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}

type LabelRowProps = {
  entry: Entry;
  label: string;
  draft: ButtonDraft;
  max: number;
  brand: string;
  onChange: (label: string) => void;
  disabled: boolean;
};

function LabelRow({ entry, label, draft, max, brand, onChange, disabled }: LabelRowProps) {
  const id = `bt-${entry.key}-en`;
  const problem = labelProblem(entry, label, max);
  const shown = fill(label.trim() || entry.default_en, entry.vars, brand, false, 'en');
  const custom = label.trim() !== '' && label.trim() !== entry.default_en;
  return (
    <div className="row bt-entry">
      <label htmlFor={id} className="rl">
        {entry.label}
        {custom ? <span className="bt-mark">изменено</span> : null}
        {!custom && draft.text.trim() && draft.text.trim() !== entry.default ? <span className="bt-mark">русский текст свой</span> : null}
      </label>
      <div className="rc">
        <input
          id={id}
          className="field"
          value={label}
          placeholder={entry.default_en}
          maxLength={max * 2}
          aria-invalid={problem !== null}
          onChange={(e) => onChange(e.target.value)}
          disabled={disabled}
          lang="en"
        />
        {entry.vars.length ? <span className="cap">Можно: {entry.vars.map((v) => `{${v.name}} — ${v.hint}`).join(', ')}</span> : null}
        {problem ? <span className="err">{problem}</span> : null}
        <span className="bt-preview-btn" aria-label="Как увидит клиент">
          <span className={`bt-tg-btn ${draft.style}`} data-hidden={draft.hidden || undefined}>
            {draft.emoji_id.trim() ? <span className="bt-icon" title="Премиум-эмодзи" aria-hidden="true" /> : null}
            {shown}
          </span>
          {label.trim() ? (
            <button type="button" className="bt-reset hov" onClick={() => onChange('')} disabled={disabled}>
              Стандартный перевод
            </button>
          ) : null}
        </span>
      </div>
    </div>
  );
}

type ButtonRowProps = {
  entry: Entry;
  draft: ButtonDraft;
  max: number;
  brand: string;
  onChange: (draft: ButtonDraft) => void;
  disabled: boolean;
};

function ButtonRow({ entry, draft, max, brand, onChange, disabled }: ButtonRowProps) {
  const id = `bt-${entry.key}`;
  const problem = buttonProblem(entry, draft, max);
  const label = fill(draft.text.trim() || entry.default, entry.vars, brand, false);
  const custom = !draftIsPlain(entry, draft);
  return (
    <div className="row bt-entry">
      <label htmlFor={id} className="rl">
        {entry.label}
        {custom ? <span className="bt-mark">изменено</span> : null}
      </label>
      <div className="rc">
        <span className="bt-btnrow">
          <input
            id={id}
            className="field"
            value={draft.text}
            placeholder={entry.default}
            maxLength={max * 2}
            aria-invalid={problem !== null}
            onChange={(e) => onChange({ ...draft, text: e.target.value })}
            disabled={disabled}
          />
          <input
            className="field mono bt-emoji-id"
            value={draft.emoji_id}
            placeholder="Код премиум-эмодзи"
            inputMode="numeric"
            aria-label={`Код премиум-эмодзи для кнопки «${entry.label}»`}
            onChange={(e) => onChange({ ...draft, emoji_id: e.target.value })}
            disabled={disabled}
          />
        </span>
        <span className="bt-btnrow">
          <span className="segs" role="group" aria-label="Цвет кнопки">
            {STYLES.map((style) => (
              <button
                key={style.value || 'plain'}
                type="button"
                aria-pressed={draft.style === style.value}
                onClick={() => onChange({ ...draft, style: style.value })}
                disabled={disabled}
              >
                {style.label}
              </button>
            ))}
          </span>
          {entry.optional ? (
            <span className="bt-show">
              <Switch on={!draft.hidden} onToggle={() => onChange({ ...draft, hidden: !draft.hidden })} label={`Показывать кнопку «${entry.label}»`} disabled={disabled} />
              <span>{draft.hidden ? 'Скрыта' : 'Показывается'}</span>
            </span>
          ) : null}
        </span>
        {entry.vars.length ? (
          <span className="cap">Можно: {entry.vars.map((v) => `{${v.name}} — ${v.hint}`).join(', ')}</span>
        ) : null}
        {problem ? <span className="err">{problem}</span> : null}
        <span className="bt-preview-btn" aria-label="Как увидит клиент">
          <span className={`bt-tg-btn ${draft.style}`} data-hidden={draft.hidden || undefined}>
            {draft.emoji_id.trim() ? <span className="bt-icon" title="Премиум-эмодзи" aria-hidden="true" /> : null}
            {label}
          </span>
          {draft.style || draft.emoji_id.trim() ? (
            <button
              type="button"
              className="bt-reset hov"
              onClick={() => onChange({ ...draft, style: '', emoji_id: '' })}
              disabled={disabled}
            >
              Без цвета и эмодзи
            </button>
          ) : null}
          {draft.text.trim() && draft.text.trim() !== entry.default ? (
            <button type="button" className="bt-reset hov" onClick={() => onChange({ ...draft, text: '' })} disabled={disabled}>
              Как было
            </button>
          ) : null}
        </span>
      </div>
    </div>
  );
}
