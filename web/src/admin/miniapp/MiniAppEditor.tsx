import { useCallback, useEffect, useLayoutEffect, useMemo, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count } from '../../lib/format';
import type { Lang } from '../../lib/i18n';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { botLink } from '../../lib/referral';
import { useSite } from '../../lib/site';
import type { Theme } from '../../lib/theme';
import {
  homeOn,
  profileBlock,
  profileOn,
  standardConfig,
  type Audience,
  type HomeBlock,
  type HomeKind,
  type MiniConfig,
  type Part,
  type PreviewScreen,
  type ProfileBlock,
  type ProfileKind,
} from '../../mini/config';
import { actionText } from '../../mini/labels';
import { CssEditor } from '../css/CssEditor';
import { cssOut, joinStyles, linesOf, placeProblems, readTexts, sameCss, type CssTexts } from '../css/cssText';
import { useCssCheck } from '../css/useCssCheck';
import { SettingsLayout } from '../pages/SettingsLayout';
import { Switch } from '../ui';
import { HeaderBody, HomeBody, ProfileBody, TabsEditor, type Kit } from './blocks';
import { CSS_PATH, MINI_PLACES, placeHint, placeMax, placeName, placeSample, type MiniPlace } from './css';
import { MINI_SNIPPETS } from './cssSnippets';
import { Grip, MoveButtons, sendImage, useDragOrder } from './fields';
import {
  canonical,
  fromView,
  HOME_NAMES,
  headerSummary,
  homeChanged,
  homeSummary,
  IMAGE_RULES,
  isStandard,
  LIMITS,
  moved,
  problemsOf,
  PROFILE_NAMES,
  profileChanged,
  profileSummary,
  resetHome,
  resetProfile,
  sameConfig,
  screenOf,
  SCREENS,
  type AdminView,
  type ImageRules,
  type Limits,
  type Screen,
  type SectionInfo,
} from './model';
import { PreviewFrame, StateSegs, ThemeSegs, useSize, useViewportHeight, type Focus } from './Preview';
import '../form.css';
import './mini-editor.css';

type Meta = { limits: Limits; rules: ImageRules; sections: SectionInfo[]; english: boolean; off: string | null; percent: number; levels: number[] };

const API = '/api/admin/settings/miniapp';
const CHROME = 182;

type Opened = Part | 'css' | 'css-tabs' | 'css-profile';

type BlockProps = {
  id: string;
  name: string;
  summary: string;
  open: boolean;
  off: boolean;
  onOpen: () => void;
  end: ReactNode;
  grip?: ReactNode;
  drag?: ReturnType<ReturnType<typeof useDragOrder>['row']>;
  mark?: string;
  bad?: boolean;
  children?: ReactNode;
};

function Block({ id, name, summary, open, off, onOpen, end, grip, drag, mark = '', bad, children }: BlockProps) {
  return (
    <div id={`mce-${id}`} className={`mce-block${open ? ' open' : ''}${off ? ' hid' : ''}${mark}`} data-drag="" {...drag}>
      <div className="mce-head-row">
        {grip ?? <span className="mce-grip-space" aria-hidden="true" />}
        <button type="button" className="mce-main" aria-expanded={open} aria-controls={`mce-${id}-body`} onClick={onOpen}>
          <span className="mce-text">
            <span className="mce-name">{name}</span>
            <span className={bad ? 'mce-sum bad' : 'mce-sum'}>{summary}</span>
          </span>
          <Icon name="chevron-down" size={18} />
        </button>
        {end}
      </div>
      {open ? (
        <div className="mce-body" id={`mce-${id}-body`}>
          {children}
        </div>
      ) : null}
    </div>
  );
}

export function SettingsMiniApp() {
  const site = useSite();
  const phone = usePhone();
  const viewport = useViewportHeight();
  const [saved, setSaved] = useState<MiniConfig | null>(null);
  const [draft, setDraft] = useState<MiniConfig | null>(null);
  const [meta, setMeta] = useState<Meta>({ limits: LIMITS, rules: IMAGE_RULES, sections: [], english: false, off: null, percent: 0, levels: [] });
  const [loadError, setLoadError] = useState<string | null>(null);
  const [cssSaved, setCssSaved] = useState<CssTexts<MiniPlace>>({});
  const [cssDraft, setCssDraft] = useState<CssTexts<MiniPlace>>({});
  const css = useCssCheck(CSS_PATH, MINI_PLACES, cssDraft);
  const resetCss = css.reset;
  const [screen, setScreen] = useState<Screen>('home');
  const [open, setOpen] = useState<Opened | null>(null);
  const [focus, setFocus] = useState<Focus | null>(null);
  const [lang, setLang] = useState<Lang>('ru');
  const [english, setEnglish] = useState<Record<string, string> | null>(null);
  const [state, setState] = useState<Audience>('active');
  const [theme, setTheme] = useState<Theme>('light');
  const [sheet, setSheet] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState('');
  const [asking, setAsking] = useState(false);
  const [wrap, setWrap] = useState<HTMLDivElement | null>(null);
  const [sheetBody, setSheetBody] = useState<HTMLDivElement | null>(null);
  const [grid, setGrid] = useState<HTMLDivElement | null>(null);
  const [gridTop, setGridTop] = useState(200);
  const { width } = useSize(wrap);
  const sheetSize = useSize(sheetBody);
  const wide = !phone && width >= 860;

  const apply = useCallback((view: AdminView) => {
    const config = fromView(view);
    setSaved(config);
    setDraft(config);
    const texts = readTexts(MINI_PLACES, view.css);
    setCssSaved(texts);
    setCssDraft(texts);
    resetCss(texts, readTexts(MINI_PLACES, view.styles));
    setMeta({
      limits: { ...LIMITS, ...(view.limits ?? {}) },
      rules: view.image ?? IMAGE_RULES,
      sections: view.sections ?? [],
      english: view.english === true,
      off: view.off ?? null,
      percent: view.friends_percent ?? 0,
      levels: view.friends_levels ?? [],
    });
  }, [resetCss]);

  useEffect(() => {
    api<AdminView>(API)
      .then(apply)
      .catch((err: unknown) => setLoadError(err instanceof ApiError ? err.message : 'Настройки мини-приложения не загрузились'));
  }, [apply]);

  useEffect(() => {
    if (!meta.english) {
      return;
    }
    let alive = true;
    import('../../lib/i18n/en')
      .then((module) => alive && setEnglish(module.EN))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [meta.english]);

  const clientTheme = site?.appearance.client_theme;
  useEffect(() => {
    if (clientTheme === 'dark' || clientTheme === 'light') {
      setTheme(clientTheme);
    }
  }, [clientTheme]);

  const shownLang: Lang = meta.english ? lang : 'ru';
  const text = useCallback((ru: string) => (shownLang === 'en' ? (english?.[ru] ?? ru) : ru), [shownLang, english]);
  const kit: Kit = useMemo(() => ({ lang: shownLang, phone, limits: meta.limits, rules: meta.rules, sections: meta.sections, text }), [shownLang, phone, meta, text]);
  const off = useMemo(() => meta.sections.filter((section) => section.off).map((section) => section.key), [meta.sections]);
  const dirty = useMemo(() => (saved && draft ? !sameConfig(saved, draft) || !sameCss(MINI_PLACES, cssDraft, cssSaved) : false), [saved, draft, cssDraft, cssSaved]);
  const blockProblems = useMemo(() => (draft ? problemsOf(draft, meta.limits) : []), [draft, meta.limits]);
  const cssProblems = useMemo(() => placeProblems(MINI_PLACES, cssDraft, css.problems, (place) => placeMax(place, meta.limits), placeName), [cssDraft, css.problems, meta.limits]);
  const problems = useMemo(() => [...blockProblems, ...cssProblems], [blockProblems, cssProblems]);
  const previewCss = useMemo(() => joinStyles(MINI_PLACES, css.styles), [css.styles]);

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

  useEffect(() => {
    if (wide) {
      setSheet(false);
    }
  }, [wide]);

  useLayoutEffect(() => {
    if (grid) {
      setGridTop(Math.round(grid.getBoundingClientRect().top + window.scrollY));
    }
  }, [grid, width, viewport, meta.off]);

  const touched = () => {
    setNote('');
    setError(null);
  };

  const patch = useCallback((next: (config: MiniConfig) => MiniConfig) => {
    setDraft((current) => (current ? next(current) : current));
    setNote('');
    setError(null);
  }, []);

  const changeHome = (block: HomeBlock) => patch((config) => ({ ...config, home: config.home.map((other) => (other.kind === block.kind ? block : other)) }));
  const changeProfile = (block: ProfileBlock) => patch((config) => ({ ...config, profile: config.profile.map((other) => (other.kind === block.kind ? block : other)) }));
  const moveHome = (from: number, to: number) => patch((config) => ({ ...config, home: moved(config.home, from, to) }));
  const moveProfile = (from: number, to: number) => patch((config) => ({ ...config, profile: moved(config.profile, from, to) }));
  const homeOrder = useDragOrder('home', moveHome);
  const profileOrder = useDragOrder('profile', moveProfile);

  const changeCss = (place: MiniPlace, next: string) => {
    setCssDraft((current) => ({ ...current, [place]: next }));
    touched();
  };

  const jump = useCallback((part: Part) => setFocus((current) => ({ part, n: (current?.n ?? 0) + 1 })), []);

  const expand = (part: Part) => {
    setOpen((current) => (current === part ? null : part));
    if (open !== part) {
      jump(part);
    }
  };

  const expandCss = (row: Opened, part: Part | null) => {
    setOpen((current) => (current === row ? null : row));
    if (part && open !== row) {
      jump(part);
    }
  };

  const onPick = useCallback((part: Part) => {
    setScreen(screenOf(part));
    setOpen(part);
    window.setTimeout(() => document.getElementById(`mce-${part}`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }), 40);
  }, []);

  const save = async () => {
    if (!draft || !dirty || problems.length || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    setNote('');
    try {
      apply(await api<AdminView>(API, { method: 'PUT', body: { ...canonical(draft), css: cssOut(MINI_PLACES, cssDraft) } }));
      setNote('Сохранено, клиенты уже видят новый вид');
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Не получилось сохранить');
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    setDraft(saved);
    setCssDraft(cssSaved);
    touched();
  };

  const resetAll = () => {
    setDraft(standardConfig());
    setCssDraft({});
    setAsking(false);
    setOpen(null);
    touched();
  };

  const head = (
    <div className="mce-head">
      <div className="head-stack">
        <h2 className="section-title">Мини-приложение</h2>
        <span className="muted">Главная, меню внизу и профиль клиента. Тарифы и цены берутся из каталога</span>
      </div>
      {site?.bot_username ? (
        <a href={botLink(site.bot_username)} target="_blank" rel="noopener noreferrer" className="mce-open">
          Открыть бота
          <Icon name="external" size={16} />
        </a>
      ) : null}
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

  if (!draft) {
    return (
      <SettingsLayout>
        {head}
        <div className="card mce-list" aria-busy="true" style={{ minHeight: 320 }} />
      </SettingsLayout>
    );
  }

  const badParts = new Set<string>([...blockProblems.map((problem) => problem.part), ...cssProblems.map((problem) => problem.place)]);
  const own = (place: MiniPlace) => Boolean(cssDraft[place]?.trim());
  const mark = (summary: string, place: MiniPlace) => (own(place) ? `Свой CSS · ${summary}` : summary);
  const sendCssImage = (file: File) => sendImage(file, meta.rules);
  const cssField = (place: MiniPlace, label: string) => (
    <CssEditor
      key={place}
      label={label}
      value={cssDraft[place] ?? ''}
      max={placeMax(place, meta.limits)}
      problem={css.problems[place] ?? null}
      imageRule={place === 'page' ? '.mn' : null}
      rows={place === 'page' ? [14, 28] : [4, 18]}
      hint={placeHint(place)}
      placeholder={placeSample(place)}
      snippets={MINI_SNIPPETS[place]}
      types={meta.rules.types}
      sendImage={sendCssImage}
      onChange={(next) => changeCss(place, next)}
    />
  );
  const cssRow = (row: Opened, place: MiniPlace, part: Part | null, label: string, empty: string) => (
    <Block
      id={row}
      name={row === 'css' ? 'Общий CSS' : label}
      summary={own(place) ? count(linesOf(cssDraft[place] ?? ''), 'строка', 'строки', 'строк') : empty}
      bad={badParts.has(place)}
      open={open === row}
      off={false}
      onOpen={() => expandCss(row, part)}
      end={null}
    >
      {cssField(place, label)}
    </Block>
  );
  const pageCss = cssRow('css', 'page', null, 'CSS мини-приложения', 'Все экраны: цвета, шрифт, кнопки');

  const homeList = (
    <div className="card mce-list">
      <Block
        id="header"
        name="Шапка"
        summary={mark(headerSummary(draft.header), 'header')}
        bad={badParts.has('header')}
        open={open === 'header'}
        off={false}
        onOpen={() => expand('header')}
        end={<span className="mce-pin">Всегда сверху</span>}
      >
        <HeaderBody header={draft.header} onChange={(header) => patch((config) => ({ ...config, header }))} />
        {cssField('header', 'CSS блока')}
      </Block>
      {draft.home.map((block, index) => {
        const kind: HomeKind = block.kind;
        const on = homeOn(block);
        const name = HOME_NAMES[kind];
        const always = kind === 'subscription';
        const bad = blockProblems.find((problem) => problem.part === kind);
        return (
          <Block
            key={kind}
            id={kind}
            name={name}
            summary={mark(on || kind === 'greeting' ? homeSummary(block, actionText) : 'Скрыт', kind)}
            bad={badParts.has(kind)}
            open={open === kind}
            off={!on}
            onOpen={() => expand(kind)}
            grip={phone ? undefined : <Grip order={homeOrder} index={index} phone={phone} name={name} />}
            drag={homeOrder.row(index)}
            mark={homeOrder.mark(index)}
            end={
              always ? (
                <span className="mce-pin">Всегда видна</span>
              ) : (
                <Switch on={on} onToggle={() => changeHome({ ...block, on: !on } as HomeBlock)} label={`Показывать блок «${name}»`} />
              )
            }
          >
            <HomeBody block={block} change={changeHome} kit={kit} />
            {bad ? <span className="err">{bad.text}</span> : null}
            {cssField(kind, 'CSS блока')}
            <div className="mce-foot">
              <MoveButtons name={name} first={index === 0} last={index === draft.home.length - 1} onUp={() => moveHome(index, index - 1)} onDown={() => moveHome(index, index + 1)} />
              {homeChanged(block) || own(kind) ? (
                <button
                  type="button"
                  className="mce-reset"
                  onClick={() => {
                    changeHome(resetHome(block));
                    changeCss(kind, '');
                  }}
                >
                  Вернуть стандартный блок
                </button>
              ) : null}
            </div>
          </Block>
        );
      })}
      {pageCss}
    </div>
  );

  const profileList = (
    <div className="card mce-list">
      {draft.profile.map((block, index) => {
        const kind: ProfileKind = block.kind;
        const on = profileOn(block);
        const name = PROFILE_NAMES[kind];
        const always = kind === 'help' || kind === 'documents' || kind === 'account';
        return (
          <Block
            key={kind}
            id={kind}
            name={name}
            summary={profileSummary(block)}
            bad={badParts.has(kind)}
            open={open === kind}
            off={!on}
            onOpen={() => expand(kind)}
            grip={phone ? undefined : <Grip order={profileOrder} index={index} phone={phone} name={name} />}
            drag={profileOrder.row(index)}
            mark={profileOrder.mark(index)}
            end={
              always ? (
                <span className="mce-pin">Всегда есть</span>
              ) : (
                <Switch on={on} onToggle={() => changeProfile({ ...block, on: !on } as ProfileBlock)} label={`Показывать «${name}» в профиле`} />
              )
            }
          >
            <ProfileBody block={block} change={changeProfile} kit={kit} percent={meta.percent} />
            <div className="mce-foot">
              <MoveButtons name={name} first={index === 0} last={index === draft.profile.length - 1} onUp={() => moveProfile(index, index - 1)} onDown={() => moveProfile(index, index + 1)} />
              {profileChanged(block) ? (
                <button type="button" className="mce-reset" onClick={() => changeProfile(resetProfile(block))}>
                  Вернуть стандартный блок
                </button>
              ) : null}
            </div>
          </Block>
        );
      })}
      {cssRow('css-profile', 'profile', null, 'CSS профиля', 'Экран «Профиль»: карточки, строки, кнопки')}
      {pageCss}
    </div>
  );

  const tabsList = (
    <>
      <TabsEditor tabs={draft.tabs} kit={kit} onChange={(tabs) => patch((config) => ({ ...config, tabs }))} />
      <div className="card mce-list">
        {cssRow('css-tabs', 'tabs', 'tabs', 'CSS меню внизу', 'Полоса, вкладки, открытая вкладка')}
        {pageCss}
      </div>
    </>
  );

  const left = screen === 'home' ? homeList : screen === 'profile' ? profileList : tabsList;
  const friendsScreen = screen === 'profile' && open === 'friends' && profileBlock(draft, 'friends').place === 'screen';
  const previewScreen: PreviewScreen = friendsScreen ? 'friends' : screen === 'profile' ? 'profile' : 'home';
  const previewHeight = Math.max(400, Math.min(844, viewport - gridTop - CHROME));
  const frame = (height: number, pick: (part: Part) => void) => (
    <PreviewFrame config={draft} css={previewCss} theme={theme} lang={shownLang} state={state} screen={previewScreen} off={off} percent={meta.percent} levels={meta.levels} focus={focus} height={height} onPick={pick} />
  );

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

  const resetLink = (
    <button type="button" className="mce-reset-all" onClick={() => setAsking(true)} disabled={busy || (isStandard(draft) && !MINI_PLACES.some(own))}>
      Вернуть стандартное
    </button>
  );
  const saveButton = (
    <button type="button" className="btn-main" onClick={() => void save()} disabled={!dirty || busy || problems.length > 0}>
      {busy ? 'Сохраняем…' : 'Сохранить'}
    </button>
  );

  return (
    <SettingsLayout>
      <div className="mce" ref={setWrap}>
        {head}
        {meta.off ? (
          <div className="card mce-off">
            <span className="mce-off-text">
              <span className="mce-name">Клиенты пока не увидят мини-приложение</span>
              <span className="muted">{meta.off}. Вид можно готовить заранее</span>
            </span>
            <Link className="b34 hov mce-off-link" to="/admin/settings/client">
              Кабинет и мини-приложение
            </Link>
          </div>
        ) : null}
        <div className="mce-bar">
          <span className="segs" role="group" aria-label="Что настраиваем">
            {SCREENS.map((item) => (
              <button
                key={item.key}
                type="button"
                aria-pressed={screen === item.key}
                onClick={() => {
                  setScreen(item.key);
                  if (item.key === 'tabs') {
                    jump('tabs');
                  }
                }}
              >
                {item.name}
              </button>
            ))}
          </span>
          {wide || meta.english ? (
            <span className="mce-bar-end">
              {wide ? <ThemeSegs theme={theme} onTheme={setTheme} /> : null}
              {meta.english ? (
                <span className="segs" role="group" aria-label="Язык текстов">
                  {(['ru', 'en'] as Lang[]).map((value) => (
                    <button key={value} type="button" aria-pressed={lang === value} onClick={() => setLang(value)}>
                      {value.toUpperCase()}
                    </button>
                  ))}
                </span>
              ) : null}
            </span>
          ) : null}
        </div>
        <div ref={setGrid} className={wide ? 'mce-grid' : 'mce-grid single'}>
          <div className="mce-left">{left}</div>
          {wide ? (
            <div className="card mce-pv">
              <div className="mce-pv-tools">
                <StateSegs state={state} onState={setState} />
              </div>
              {frame(previewHeight, onPick)}
            </div>
          ) : null}
        </div>
        {phone ? <div className="mce-under">{resetLink}</div> : null}
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
              {resetLink}
            </>
          )}
        </div>
      </div>
      {sheet ? (
        <div className="mce-sheet" role="dialog" aria-modal="true" aria-label="Превью мини-приложения">
          <div className="mce-sheet-head">
            <span className="mce-sheet-title">Превью</span>
            <button type="button" className="mce-icon hov" aria-label="Закрыть превью" onClick={() => setSheet(false)} autoFocus>
              <Icon name="close" size={20} />
            </button>
            <div className="mce-pv-tools">
              <StateSegs state={state} onState={setState} />
              <ThemeSegs theme={theme} onTheme={setTheme} />
            </div>
          </div>
          <div className="mce-sheet-body" ref={setSheetBody}>
            {sheetSize.height > 0
              ? frame(sheetSize.height, (part) => {
                  setSheet(false);
                  onPick(part);
                })
              : null}
          </div>
        </div>
      ) : null}
      {asking ? (
        <div className="dialog-back" role="presentation" onClick={() => setAsking(false)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="mce-reset-h" onClick={(event) => event.stopPropagation()}>
            <h2 id="mce-reset-h">Вернуть стандартный вид?</h2>
            <p className="muted">Порядок, показ и тексты станут как при установке, свои баннеры, ссылки и CSS уберутся. Клиенты увидят это после сохранения.</p>
            <div className="dialog-actions">
              <button type="button" className="btn-main" onClick={resetAll} autoFocus>
                Вернуть
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
