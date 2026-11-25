import { useEffect, useLayoutEffect, useMemo, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { MemoryRouter, Route, Routes } from 'react-router';
import { setAnswer } from '../../lib/api';
import { applyLang, type Lang } from '../../lib/i18n';
import { usePhone } from '../../lib/media';
import { usePreviewStyles } from '../../lib/ownStyles';
import { useSurface } from '../../lib/theme';
import { CabinetShell, type CabinetCtx } from '../CabinetApp';
import { Login } from '../Login';
import { isPart, type Part } from '../parts';
import { demoOf } from './demo';
import { CABINET_PREVIEW, SCREEN_PATHS, type CabinetDraft } from './model';
import './preview.css';

const nothing = () => undefined;
const later = async () => undefined;

export function CabinetPreview() {
  const phone = usePhone();
  const [draft, setDraft] = useState<CabinetDraft | null>(null);
  const [focus, setFocus] = useState<{ part: Part; at: number } | null>(null);
  const [applied, setApplied] = useState<Lang | null>(null);
  const [ready, setReady] = useState(false);
  const wanted = draft?.lang ?? null;
  const screen = draft?.screen ?? 'subscription';
  usePreviewStyles(typeof draft?.css === 'string' ? draft.css : '');
  useSurface(screen === 'login' || !phone ? 'cabinet' : 'cabinet-phone', draft?.theme ?? 'light', draft?.site.appearance.client === 'mono' ? 'b' : 'a');

  useEffect(() => {
    const origin = window.location.origin;
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== origin || event.source !== window.parent) {
        return;
      }
      const data = event.data as CabinetDraft | null;
      if (!data || data.type !== CABINET_PREVIEW.draft) {
        return;
      }
      setDraft(data);
      if (data.focus) {
        setFocus({ part: data.focus, at: Date.now() });
      }
    };
    window.addEventListener('message', onMessage);
    if (window.parent !== window) {
      window.parent.postMessage({ type: CABINET_PREVIEW.ready }, origin);
    }
    return () => window.removeEventListener('message', onMessage);
  }, []);

  useEffect(() => {
    if (!wanted) {
      return;
    }
    let alive = true;
    const done = () => {
      if (alive) {
        setApplied(wanted);
      }
    };
    applyLang(wanted).then(done, done);
    return () => {
      alive = false;
    };
  }, [wanted]);

  const site = draft?.site ?? null;
  const state = draft?.state ?? 'active';
  const shownLang: Lang = applied ?? 'ru';
  const siteKey = site ? JSON.stringify(site) : '';
  const demo = useMemo(() => (site ? demoOf(site, shownLang, state) : null), [siteKey, shownLang, state]);
  useLayoutEffect(() => {
    if (!demo) {
      return;
    }
    setAnswer(demo.answer);
    setReady(true);
    return () => setAnswer(null);
  }, [demo]);

  useEffect(() => {
    window.scrollTo({ top: 0 });
  }, [screen]);

  useEffect(() => {
    if (!focus) {
      return;
    }
    const found = [...document.querySelectorAll<HTMLElement>(`[data-part="${focus.part}"]`)];
    const [first] = found;
    if (!first) {
      return;
    }
    const box = first.getBoundingClientRect();
    if (box.top < 0 || box.top > window.innerHeight - 160) {
      window.scrollTo({ top: Math.max(0, box.top + window.scrollY - 24), behavior: 'smooth' });
    }
    for (const node of found) {
      node.classList.add('cb-picked');
    }
    const timer = window.setTimeout(() => {
      for (const node of found) {
        node.classList.remove('cb-picked');
      }
    }, 1400);
    return () => {
      window.clearTimeout(timer);
      for (const node of found) {
        node.classList.remove('cb-picked');
      }
    };
  }, [focus]);

  const theme = draft?.theme ?? 'light';
  const ctx = useMemo<CabinetCtx | null>(
    () =>
      demo && site
        ? { me: demo.me, site, reload: later, logout: later, theme, flipTheme: nothing, phone, lang: shownLang, nextLang: null, switchLang: nothing, unread: 1, setUnread: nothing }
        : null,
    [demo, site, theme, phone, shownLang],
  );

  if (!site || !ctx || !draft || applied === null || !ready) {
    return <div className="cb-blank" aria-busy="true" />;
  }

  const onClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const part = (event.target as HTMLElement).closest('[data-part]')?.getAttribute('data-part');
    if (isPart(part) && window.parent !== window) {
      window.parent.postMessage({ type: CABINET_PREVIEW.pick, part }, window.location.origin);
    }
  };

  const path = screen === 'login' ? '/cabinet' : SCREEN_PATHS[screen];
  return (
    <div className="cb-preview" onClickCapture={onClick}>
      <MemoryRouter key={`${screen}-${state}-${shownLang}`} initialEntries={[path]}>
        {screen === 'login' ? (
          <Login site={{ ...site, appearance: { ...site.appearance, client_theme: theme } }} onDone={nothing} />
        ) : (
          <Routes>
            <Route path="/cabinet/*" element={<CabinetShell ctx={ctx} />} />
          </Routes>
        )}
      </MemoryRouter>
    </div>
  );
}
