import { useCallback, useEffect, useLayoutEffect, useMemo, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api, ApiError, upload } from '../../lib/api';
import { count } from '../../lib/format';
import type { Lang } from '../../lib/i18n';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { useSite } from '../../lib/site';
import type { Theme } from '../../lib/theme';
import type { Site } from '../../lib/types';
import type { Part } from '../../cabinet/parts';
import type { PreviewScreen, PreviewState } from '../../cabinet/preview/model';
import { CssEditor } from '../css/CssEditor';
import { cssOut, joinStyles, linesOf, placeProblems, readTexts, sameCss, type CssTexts } from '../css/cssText';
import { useCssCheck } from '../css/useCssCheck';
import { weight } from '../miniapp/fields';
import { IMAGE_RULES, type ImageRules } from '../miniapp/model';
import { ThemeSegs, useSize, useViewportHeight } from '../miniapp/Preview';
import { SettingsLayout } from '../pages/SettingsLayout';
import { CABINET_PLACES, CSS_PATH, PLACE_ABOUT, PLACE_NAMES, placeHint, placeOf, placeSample, placeTitle, SCREEN_NAMES, screenOf, SNIPPETS, STATES, type CabinetPlace } from './places';
import { PreviewFrame, type Device, type Focus } from './Preview';
import '../form.css';
import '../miniapp/mini-editor.css';
import './cabinet-editor.css';

type View = { css?: unknown; styles?: unknown; limits?: { page_css: number; block_css: number }; image?: ImageRules; english?: boolean; off?: string | null };
type Meta = { limits: { page_css: number; block_css: number }; rules: ImageRules; english: boolean; off: string | null };

const API = '/api/admin/settings/cabinet';
const LIMITS = { page_css: 50_000, block_css: 10_000 };

async function sendImage(file: File, rules: ImageRules): Promise<{ url: string }> {
  if (!rules.types.includes(file.type)) {
    throw new ApiError(0, 'cabinet_image.format', 'Подходят только PNG, JPEG и WebP');
  }
  if (file.size > rules.max_bytes) {
    throw new ApiError(0, 'cabinet_image.too_heavy', `Файл больше ${weight(rules.max_bytes)}: уменьшите картинку`);
  }
  return upload<{ url: string }>('/api/admin/settings/cabinet/images', new Blob([file], { type: file.type }));
}

function previewSite(site: Site): Site {
  return { ...site, cabinet: null };
}

type ToolsProps = {
  screen: PreviewScreen;
  state: PreviewState;
  device: Device;
  theme: Theme;
  lang: Lang;
  english: boolean;
  onScreen: (screen: PreviewScreen) => void;
  onState: (state: PreviewState) => void;
  onDevice: (device: Device) => void;
  onTheme: (theme: Theme) => void;
  onLang: (lang: Lang) => void;
  onFull?: () => void;
};

function Tools({ screen, state, device, theme, lang, english, onScreen, onState, onDevice, onTheme, onLang, onFull }: ToolsProps) {
  return (
    <div className="cbe-tools">
      <select className="cbe-pick" aria-label="Экран кабинета" value={screen} onChange={(e) => onScreen(e.target.value as PreviewScreen)}>
        {SCREEN_NAMES.map((item) => (
          <option key={item.key} value={item.key}>
            Экран: {item.name}
          </option>
        ))}
      </select>
      <select className="cbe-pick" aria-label="Какого клиента показать" value={state} onChange={(e) => onState(e.target.value as PreviewState)}>
        {STATES.map((item) => (
          <option key={item.key} value={item.key}>
            Клиент: {item.name}
          </option>
        ))}
      </select>
      <span className="segs" role="group" aria-label="Превью на устройстве">
        <button type="button" aria-pressed={device === 'desktop'} onClick={() => onDevice('desktop')}>
          Компьютер
        </button>
        <button type="button" aria-pressed={device === 'phone'} onClick={() => onDevice('phone')}>
          Телефон
        </button>
      </span>
      <ThemeSegs theme={theme} onTheme={onTheme} />
      {english ? (
        <span className="segs" role="group" aria-label="Язык превью">
          {(['ru', 'en'] as Lang[]).map((value) => (
            <button key={value} type="button" aria-pressed={lang === value} onClick={() => onLang(value)}>
              {value.toUpperCase()}
            </button>
          ))}
        </span>
      ) : null}
      {onFull ? (
        <button type="button" className="btn-line hov cbe-full" onClick={onFull}>
          Крупнее
        </button>
      ) : null}
    </div>
  );
}

export function SettingsCabinet() {
  const site = useSite();
  const phone = usePhone();
  const viewport = useViewportHeight();
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [meta, setMeta] = useState<Meta>({ limits: LIMITS, rules: IMAGE_RULES, english: false, off: null });
  const [saved, setSaved] = useState<CssTexts<CabinetPlace>>({});
  const [draft, setDraft] = useState<CssTexts<CabinetPlace>>({});
  const css = useCssCheck(CSS_PATH, CABINET_PLACES, draft);
  const resetCss = css.reset;
  const [open, setOpen] = useState<CabinetPlace | null>(null);
  const [screen, setScreen] = useState<PreviewScreen>('subscription');
  const [state, setState] = useState<PreviewState>('active');
  const [device, setDevice] = useState<Device>('desktop');
  const [theme, setTheme] = useState<Theme>('light');
  const [lang, setLang] = useState<Lang>('ru');
  const [focus, setFocus] = useState<Focus | null>(null);
  const [sheet, setSheet] = useState(false);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState('');
  const [wrap, setWrap] = useState<HTMLDivElement | null>(null);
  const [grid, setGrid] = useState<HTMLDivElement | null>(null);
  const [sheetBody, setSheetBody] = useState<HTMLDivElement | null>(null);
  const [gridTop, setGridTop] = useState(170);
  const { width } = useSize(wrap);
  const sheetSize = useSize(sheetBody);
  const wide = !phone && width >= 760;

  const apply = useCallback(
    (view: View) => {
      const texts = readTexts(CABINET_PLACES, view.css);
      setSaved(texts);
      setDraft(texts);
      resetCss(texts, readTexts(CABINET_PLACES, view.styles));
      setMeta({ limits: view.limits ?? LIMITS, rules: view.image ?? IMAGE_RULES, english: view.english === true, off: view.off ?? null });
      setLoaded(true);
    },
    [resetCss],
  );

  useEffect(() => {
    api<View>(API)
      .then(apply)
      .catch((err: unknown) => setLoadError(err instanceof ApiError ? err.message : 'Настройки кабинета не загрузились'));
  }, [apply]);

  const clientTheme = site?.appearance.client_theme;
  useEffect(() => {
    if (clientTheme === 'dark' || clientTheme === 'light') {
      setTheme(clientTheme);
    }
  }, [clientTheme]);

  const max = useCallback((place: CabinetPlace) => (place === 'page' ? meta.limits.page_css : meta.limits.block_css), [meta.limits]);
  const dirty = useMemo(() => !sameCss(CABINET_PLACES, draft, saved), [draft, saved]);
  const problems = useMemo(() => placeProblems(CABINET_PLACES, draft, css.problems, max, placeTitle), [draft, css.problems, max]);
  const previewCss = useMemo(() => joinStyles(CABINET_PLACES, css.styles), [css.styles]);
  const shownLang: Lang = meta.english ? lang : 'ru';

  useEffect(() => {
    if (!dirty) {
      return;
    }
    const onLeave = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', onLeave);
    return () => window.removeEventListener('beforeunload', onLeave);
  }, [dirty]);

  useEffect(() => {
    if (!sheet && !asking) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setSheet(false);
        setAsking(false);
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [sheet, asking]);

  useLayoutEffect(() => {
    if (grid) {
      setGridTop(Math.round(grid.getBoundingClientRect().top + window.scrollY));
    }
  }, [grid, width, viewport, meta.off, loaded]);

  const touched = () => {
    setNote('');
    setError(null);
  };

  const change = (place: CabinetPlace, next: string) => {
    setDraft((current) => ({ ...current, [place]: next }));
    touched();
  };

  const choose = (place: CabinetPlace) => {
    const closing = open === place;
    setOpen(closing ? null : place);
    if (!closing) {
      setScreen((now) => screenOf(place, now));
      if (place !== 'page') {
        setFocus((now) => ({ part: place, n: (now?.n ?? 0) + 1 }));
      }
    }
  };

  const onScreen = (next: PreviewScreen) => {
    setScreen(next);
    setOpen((now) => (now === null || now === 'page' || now === 'header' || now === 'footer' ? now : placeOf(next)));
  };

  const onPick = useCallback((part: Part) => {
    setOpen(part);
    window.setTimeout(() => document.getElementById(`cbe-${part}`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }), 40);
  }, []);

  const save = async () => {
    if (!dirty || problems.length || busy) {
      return;
    }
    setBusy(true);
    touched();
    try {
      apply(await api<View>(API, { method: 'PUT', body: { css: cssOut(CABINET_PLACES, draft) } }));
      setNote('Сохранено, клиенты уже видят новый вид');
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Не получилось сохранить');
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    setDraft(saved);
    touched();
  };

  const clearAll = () => {
    setDraft({});
    setAsking(false);
    touched();
  };

  const head = (
    <div className="mce-head">
      <div className="head-stack">
        <h2 className="section-title">Кабинет на сайте</h2>
        <span className="muted">Свой CSS для личного кабинета клиента. Лендинг, мини-приложение и админка не изменятся</span>
      </div>
      <a href="/cabinet" target="_blank" rel="noopener noreferrer" className="mce-open">
        Открыть кабинет
        <Icon name="external" size={16} />
      </a>
    </div>
  );

  if (loadError) {
    return (
      <SettingsLayout>
        {head}
        <div className="card card-pad">
          <span className="bad" role="alert">
            {loadError}
          </span>
        </div>
      </SettingsLayout>
    );
  }

  if (!loaded) {
    return (
      <SettingsLayout>
        {head}
        <div className="card mce-list" aria-busy="true" style={{ minHeight: 320 }} />
      </SettingsLayout>
    );
  }

  const bad = new Set(problems.map((problem) => problem.place));
  const tools = (onFull?: () => void) => (
    <Tools
      screen={screen}
      state={state}
      device={device}
      theme={theme}
      lang={shownLang}
      english={meta.english}
      onScreen={onScreen}
      onState={setState}
      onDevice={setDevice}
      onTheme={setTheme}
      onLang={setLang}
      onFull={onFull}
    />
  );
  const frame = (height: number) =>
    site ? (
      <PreviewFrame site={previewSite(site)} css={previewCss} device={device} theme={theme} lang={shownLang} state={state} screen={screen} focus={focus} height={height} onPick={onPick} />
    ) : null;

  let status: ReactNode;
  if (error) {
    status = (
      <span role="status" className="bad">
        {error}
      </span>
    );
  } else if (problems[0]) {
    status = (
      <span role="status" className="bad">
        {problems[0].text}
        {problems.length > 1 ? ` и ещё ${count(problems.length - 1, 'ошибка', 'ошибки', 'ошибок')}` : ''}
      </span>
    );
  } else {
    status = <span role="status">{dirty ? 'Клиенты увидят изменения после сохранения' : note}</span>;
  }
  const quiet = !error && !problems.length && !dirty && !note;
  const filled = CABINET_PLACES.some((place) => draft[place]?.trim());
  const clearLink = (
    <button type="button" className="mce-reset-all" onClick={() => setAsking(true)} disabled={busy || !filled}>
      Убрать весь CSS
    </button>
  );
  const saveButton = (
    <button type="button" className="btn-main" onClick={() => void save()} disabled={!dirty || busy || problems.length > 0}>
      {busy ? 'Сохраняем…' : 'Сохранить'}
    </button>
  );

  return (
    <SettingsLayout>
      <div className="mce cbe" ref={setWrap}>
        {head}
        {meta.off ? (
          <div className="card mce-off">
            <span className="mce-off-text">
              <span className="mce-name">Клиенты пока не войдут в кабинет</span>
              <span className="muted">{meta.off}. Вид можно готовить заранее</span>
            </span>
            <Link className="b34 hov mce-off-link" to="/admin/settings/client">
              Кабинет и мини-приложение
            </Link>
          </div>
        ) : null}
        <div ref={setGrid} className={wide ? 'cbe-grid' : 'cbe-grid single'}>
          <div className="card mce-list">
            {CABINET_PLACES.map((place) => {
              const opened = open === place;
              const text = draft[place]?.trim();
              return (
                <div key={place} id={`cbe-${place}`} className={opened ? 'mce-block open' : 'mce-block'}>
                  <div className="mce-head-row">
                    <button type="button" className="mce-main" aria-expanded={opened} aria-controls={`cbe-${place}-body`} onClick={() => choose(place)}>
                      <span className="mce-text">
                        <span className="mce-name">{PLACE_NAMES[place]}</span>
                        <span className={bad.has(place) ? 'mce-sum bad' : 'mce-sum'}>{text ? `Свой CSS · ${count(linesOf(text), 'строка', 'строки', 'строк')}` : PLACE_ABOUT[place]}</span>
                      </span>
                      <Icon name="chevron-down" size={18} />
                    </button>
                  </div>
                  {opened ? (
                    <div className="mce-body" id={`cbe-${place}-body`}>
                      <CssEditor
                        label={place === 'page' ? 'CSS кабинета' : 'CSS раздела'}
                        value={draft[place] ?? ''}
                        max={max(place)}
                        problem={css.problems[place] ?? null}
                        imageRule={place === 'page' ? '.cb' : null}
                        rows={place === 'page' ? [14, 28] : [8, 18]}
                        hint={placeHint(place)}
                        placeholder={placeSample(place)}
                        snippets={SNIPPETS[place]}
                        types={meta.rules.types}
                        sendImage={(file) => sendImage(file, meta.rules)}
                        onChange={(next) => change(place, next)}
                      />
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
          {wide ? (
            <div className="card mce-pv">
              {tools(() => setSheet(true))}
              {frame(Math.max(360, viewport - gridTop - 208))}
            </div>
          ) : null}
        </div>
        {phone ? <div className="mce-under">{clearLink}</div> : null}
        <div className="mce-save">
          {phone ? (
            <>
              <span className={quiet ? 'mce-save-status quiet' : 'mce-save-status'}>{status}</span>
              <span className="mce-save-btns">
                {dirty ? (
                  <button type="button" className="btn-line hov" onClick={cancel} disabled={busy}>
                    Отменить
                  </button>
                ) : null}
                <button type="button" className="btn-line hov" onClick={() => setSheet(true)}>
                  Превью
                </button>
                {saveButton}
              </span>
            </>
          ) : (
            <>
              {saveButton}
              <button type="button" className="btn-line hov" onClick={cancel} disabled={!dirty || busy}>
                Отменить
              </button>
              {wide ? null : (
                <button type="button" className="btn-line hov" onClick={() => setSheet(true)}>
                  Превью
                </button>
              )}
              {status}
              <span className="mce-grow" />
              {clearLink}
            </>
          )}
        </div>
      </div>
      {sheet ? (
        <div className="mce-sheet" role="dialog" aria-modal="true" aria-label="Превью кабинета">
          <div className="mce-sheet-head">
            <span className="mce-sheet-title">Превью</span>
            <button type="button" className="mce-icon hov" aria-label="Закрыть превью" onClick={() => setSheet(false)} autoFocus>
              <Icon name="close" size={20} />
            </button>
            {tools()}
          </div>
          <div className="mce-sheet-body" ref={setSheetBody}>
            {sheetSize.height > 0 ? frame(sheetSize.height) : null}
          </div>
        </div>
      ) : null}
      {asking ? (
        <div className="dialog-back" role="presentation" onClick={() => setAsking(false)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="cbe-clear-h" onClick={(event) => event.stopPropagation()}>
            <h2 id="cbe-clear-h">Убрать весь CSS кабинета?</h2>
            <p className="muted">Кабинет станет как при установке. Клиенты увидят это после сохранения.</p>
            <div className="dialog-actions">
              <button type="button" className="btn-main" onClick={clearAll} autoFocus>
                Убрать
              </button>
              <button type="button" className="btn-line hov" onClick={() => setAsking(false)}>
                Отменить
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </SettingsLayout>
  );
}
