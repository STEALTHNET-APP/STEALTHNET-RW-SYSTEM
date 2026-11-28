import { useEffect, useRef, useState } from 'react';
import { Link, useParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { pathPart } from '../../lib/apiPath';
import { useSession } from '../session';
import { extLink, type PageToken } from './ext';
import { THEME_MESSAGE, withTheme } from './frame';
import './modules.css';
import './ext.css';

const SANDBOX = 'allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals';
const TOKEN_MESSAGE = 'stealthnet:token';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function ExtPage() {
  const { id = '' } = useParams();
  return <ExtPageBody key={id} id={id} />;
}

function ExtPageBody({ id }: { id: string }) {
  const { can, theme } = useSession();
  const frame = useRef<HTMLIFrameElement>(null);
  const [page, setPage] = useState<PageToken | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api<PageToken>(`/api/admin/ext-modules/${pathPart(id)}/page-token`, { method: 'POST', body: {} })
      .then((next) => {
        if (alive) {
          setPage(next);
        }
      })
      .catch((err) => {
        if (alive) {
          setError(failText(err, 'Страница модуля не открылась'));
        }
      });
    return () => {
      alive = false;
    };
  }, [id]);

  useEffect(() => {
    if (!page) {
      return;
    }
    const expected = page.proxied ? 'null' : page.origin;
    const reply = page.proxied ? '*' : page.origin;
    const answer = async (event: MessageEvent) => {
      const target = frame.current?.contentWindow;
      if (!target || event.source !== target || event.origin !== expected) {
        return;
      }
      const data = event.data as { type?: unknown } | null;
      if (!data || data.type !== TOKEN_MESSAGE) {
        return;
      }
      try {
        const fresh = await api<PageToken>(`/api/admin/ext-modules/${pathPart(id)}/page-token`, { method: 'POST', body: {} });
        target.postMessage({ type: TOKEN_MESSAGE, token: fresh.token, expires_at: fresh.expires_at }, reply);
      } catch {
        target.postMessage({ type: TOKEN_MESSAGE, token: null, expires_at: null }, reply);
      }
    };
    const listener = (event: MessageEvent) => void answer(event);
    window.addEventListener('message', listener);
    return () => window.removeEventListener('message', listener);
  }, [id, page]);

  useEffect(() => {
    if (!page) {
      return;
    }
    frame.current?.contentWindow?.postMessage({ type: THEME_MESSAGE, theme }, page.proxied ? '*' : page.origin);
  }, [page, theme]);

  const [themed, setThemed] = useState<string | null>(null);
  useEffect(() => {
    if (page) {
      setThemed((current) => current ?? withTheme(page.url, theme));
    }
  }, [page, theme]);

  const isolated = page !== null && (page.proxied || page.origin === window.location.origin);
  const title = page?.title ?? 'Страница модуля';
  const where = page?.proxied ? 'через панель' : (page?.origin ?? '');

  return (
    <div className="md ext-frame-page">
      {can('modules') ? (
        <Link to={extLink(id)} className="md-back">
          Карточка модуля
        </Link>
      ) : null}
      <div className="md-title">
        <h1 className="page-title tight">{title}</h1>
        {page ? (
          <span className="md-where">
            Страница внешнего модуля «{page.module}» · {where}
          </span>
        ) : null}
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {page && themed ? (
        <iframe
          ref={frame}
          className="card ext-frame"
          title={title}
          src={themed}
          sandbox={isolated ? SANDBOX : `${SANDBOX} allow-same-origin`}
          referrerPolicy="no-referrer"
        />
      ) : !error ? (
        <p className="muted">Открываем страницу модуля…</p>
      ) : null}
    </div>
  );
}
