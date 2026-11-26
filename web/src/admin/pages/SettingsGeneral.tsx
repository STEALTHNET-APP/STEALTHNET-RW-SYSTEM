import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { BrandMark } from '../../lib/brand';
import { telegramLink } from '../../lib/support';
import type { Currency } from '../../lib/types';
import { useSession } from '../session';
import { fromRaw, toRaw } from '../tariffs/model';
import { Switch } from '../ui';
import '../form.css';
import { MaintenanceSection, maintenanceChanged, maintenanceForm, maintenanceProblem, saveMaintenance, type MaintenanceForm, type MaintenanceView } from './MaintenanceSettings';
import { ProxySection, proxyChanged, proxyForm, proxyProblem, saveProxy, type ProxyForm, type ProxyView } from './ProxySettings';
import { SettingsLayout } from './SettingsLayout';

type Core = {
  settings: {
    brand_name: string;
    support_url: string | null;
    support_mode: SupportMode;
    public_url: string | null;
    invoice_ttl_minutes: number;
    remna_utc_offset_minutes: number;
    currency: Currency;
    topup: { min_minor: number; max_minor: number };
  };
};

type SupportMode = 'tickets' | 'link' | 'both';

const SUPPORT_MODES: Array<[SupportMode, string]> = [
  ['tickets', 'Обращения'],
  ['link', 'Своя ссылка'],
  ['both', 'Обращения и ссылка'],
];

const SUPPORT_TEXT: Record<SupportMode, string> = {
  tickets: 'Клиенты пишут в боте, кабинете и мини-приложении, а вы отвечаете в разделе «Поддержка».',
  link: 'Кнопки «Поддержка» и «Помощь» везде открывают вашу ссылку. Новые обращения не принимаются.',
  both: 'Обращения работают как обычно, а рядом с ними — кнопка с вашей ссылкой.',
};

type SiteData = { title: string | null; description: string | null; indexing: boolean; title_max: number; description_max: number };

type Work = {
  brand: string;
  support: string;
  supportMode: SupportMode;
  ttl: string;
  offset: number;
  min: string;
  max: string;
  title: string;
  desc: string;
  indexing: boolean;
};

function chars(text: string): number {
  return [...text.trim().replace(/\s+/g, ' ')].length;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

const OFFSETS = Array.from({ length: (14 * 60 * 2) / 30 + 1 }, (_, i) => -14 * 60 + i * 30);

function offsetLabel(minutes: number): string {
  const sign = minutes < 0 ? '−' : '+';
  const abs = Math.abs(minutes);
  const h = String(Math.floor(abs / 60)).padStart(2, '0');
  const m = String(abs % 60).padStart(2, '0');
  return `UTC${sign}${h}:${m}`;
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function SettingsGeneral() {
  const { me, reload } = useSession();
  const [core, setCore] = useState<Core['settings'] | null>(null);
  const [limits, setLimits] = useState<{ title: number; desc: number }>({ title: 70, desc: 300 });
  const [base, setBase] = useState<Work | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [proxyView, setProxyView] = useState<ProxyView | null>(null);
  const [proxy, setProxy] = useState<ProxyForm | null>(null);
  const [maintView, setMaintView] = useState<MaintenanceView | null>(null);
  const [maint, setMaint] = useState<MaintenanceForm | null>(null);
  const scrolled = useRef(false);

  const load = useCallback(async () => {
    try {
      const [data, site, egress, works] = await Promise.all([
        api<Core>('/api/admin/settings/core'),
        api<SiteData>('/api/admin/settings/site'),
        api<ProxyView>('/api/admin/settings/proxy'),
        api<MaintenanceView>('/api/admin/settings/maintenance'),
      ]);
      const s = data.settings;
      const next: Work = {
        brand: s.brand_name,
        support: s.support_url ?? '',
        supportMode: s.support_mode ?? (s.support_url ? 'both' : 'tickets'),
        ttl: String(s.invoice_ttl_minutes),
        offset: s.remna_utc_offset_minutes,
        min: toRaw(s.topup.min_minor, s.currency),
        max: toRaw(s.topup.max_minor, s.currency),
        title: site.title ?? '',
        desc: site.description ?? '',
        indexing: site.indexing,
      };
      setLimits({ title: site.title_max, desc: site.description_max });
      setProxyView(egress);
      setProxy(proxyForm(egress));
      setMaintView(works);
      setMaint(maintenanceForm(works));
      setCore(s);
      setBase(next);
      setWork(next);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Настройки не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const target = window.location.hash.slice(1);
    if (proxyView && maintView && !scrolled.current && (target === 'proxy' || target === 'maintenance')) {
      scrolled.current = true;
      document.getElementById(target)?.scrollIntoView({ block: 'start' });
    }
  }, [proxyView, maintView]);

  if (!core || !work || !base || !proxy || !proxyView || !maint || !maintView) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем настройки…</p>
        )}
      </SettingsLayout>
    );
  }

  const currency = core.currency;
  const proxyDirty = proxyChanged(proxy, proxyView);
  const proxyBad = proxyDirty ? proxyProblem(proxy) : null;
  const maintBase = maintenanceForm(maintView);
  const maintDirty = maintenanceChanged(maint, maintBase);
  const maintBad = maintDirty ? maintenanceProblem(maint, maintBase, maintView) : null;
  const dirty = JSON.stringify(work) !== JSON.stringify(base) || proxyDirty || maintDirty;
  const edit = (patch: Partial<Work>) => {
    setWork({ ...work, ...patch });
    setStatus(null);
  };
  const brandBad = work.brand.trim().length === 0;
  const supportUrl = telegramLink(work.support);
  const needsLink = work.supportMode !== 'tickets';
  const supportBad = needsLink ? supportUrl === null : work.support.trim() !== '' && supportUrl === null;
  const ttl = Number.parseInt(work.ttl, 10);
  const ttlBad = Number.isNaN(ttl) || ttl < 5 || ttl > 24 * 60;
  const min = fromRaw(work.min, currency);
  const max = fromRaw(work.max, currency);
  const topupBad = min === 'bad' || max === 'bad' || min === null || max === null || min <= 0 || max < min;
  const titleLen = chars(work.title);
  const descLen = chars(work.desc);
  const titleBad = titleLen > limits.title;
  const descBad = descLen > limits.desc;
  const siteDirty = work.title !== base.title || work.desc !== base.desc || work.indexing !== base.indexing;

  const save = async () => {
    if (!dirty || busy) {
      return;
    }
    if (brandBad || supportBad || ttlBad || topupBad || titleBad || descBad) {
      setStatus({ text: 'Исправьте поля, отмеченные красным', bad: true });
      return;
    }
    if (maintBad) {
      setStatus({ text: maintBad, bad: true });
      return;
    }
    if (proxyBad) {
      setStatus({ text: proxyBad, bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      const body: Record<string, unknown> = {};
      if (work.brand !== base.brand) {
        body.brand_name = work.brand.trim();
      }
      if (work.support !== base.support) {
        body.support_url = supportUrl ?? '';
      }
      if (work.supportMode !== base.supportMode) {
        body.support_mode = work.supportMode;
      }
      if (work.ttl !== base.ttl) {
        body.invoice_ttl_minutes = ttl;
      }
      if (work.offset !== base.offset) {
        body.remna_utc_offset_minutes = work.offset;
      }
      if (work.min !== base.min || work.max !== base.max) {
        body.topup = { min_minor: min, max_minor: max };
      }
      if (Object.keys(body).length > 0) {
        await api('/api/admin/settings/core', { method: 'PUT', body });
      }
      if (siteDirty) {
        await api('/api/admin/settings/site', {
          method: 'PUT',
          body: { title: work.title, description: work.desc, indexing: work.indexing },
        });
      }
      if (proxyDirty) {
        await saveProxy(proxy);
      }
      const switched = maintDirty ? await saveMaintenance(maint, maintBase) : false;
      await load();
      if (work.brand !== base.brand || switched) {
        await reload();
      }
      setStatus({ text: 'Сохранено', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const unit = currency === 'RUB' ? '₽' : '$';

  const resetProxy = async () => {
    setBusy(true);
    setStatus(null);
    try {
      const next = await api<ProxyView>('/api/admin/settings/proxy/reset', { method: 'POST' });
      setProxyView(next);
      setProxy(proxyForm(next));
      setStatus({ text: 'Прокси снова берётся из deploy/.env', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки прокси не сбросились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsLayout>
      <section className="sec" aria-labelledby="gen-h">
        <div className="sec-head">
          <h2 id="gen-h" className="section-title">
            Сервис
          </h2>
          <span className="muted">Так сервис называется в боте, кабинете, мини-приложении и письмах</span>
        </div>
        <div className="row">
          <label htmlFor="gen-brand" className="rl">
            Название
          </label>
          <div className="rc">
            <input id="gen-brand" className="field" maxLength={64} value={work.brand} aria-invalid={brandBad} onChange={(e) => edit({ brand: e.target.value })} disabled={busy} style={{ maxWidth: 360 }} />
            {brandBad ? <span className="err">Название не может быть пустым</span> : null}
          </div>
        </div>
        <div className="row">
          <span className="rl">Адрес сервиса</span>
          <div className="rc">
            {core.public_url ? (
              <>
                <span className="mono" style={{ paddingTop: 10 }}>
                  {core.public_url}
                </span>
                <span className="cap">Задаётся при установке вместе с сертификатом, сменить — командой stealthnet domain на сервере</span>
              </>
            ) : (
              <>
                <span className="warn-line" style={{ paddingTop: 10 }}>
                  Не задан
                </span>
                <span className="cap">
                  Без него в письмах нет ссылки «Отписаться», а платёжки не получат адрес для уведомлений. Задайте домен на сервере командой stealthnet domain
                </span>
              </>
            )}
          </div>
        </div>
      </section>

      <MaintenanceSection
        view={maintView}
        form={maint}
        onChange={(next) => {
          setMaint(next);
          setStatus(null);
        }}
        busy={busy}
      />

      <section className="sec" aria-labelledby="sup-h">
        <div className="sec-head">
          <h2 id="sup-h" className="section-title">
            Поддержка
          </h2>
          <span className="muted">Куда ведут кнопки «Поддержка» и «Помощь» в боте, кабинете и мини-приложении</span>
        </div>
        <div className="row">
          <span className="rl">Как клиенты пишут</span>
          <div className="rc">
            <span role="group" aria-label="Как клиенты пишут в поддержку" className="segs">
              {SUPPORT_MODES.map(([key, label]) => (
                <button key={key} type="button" aria-pressed={work.supportMode === key} onClick={() => edit({ supportMode: key })} disabled={busy}>
                  {label}
                </button>
              ))}
            </span>
            <span className="cap">{SUPPORT_TEXT[work.supportMode]}</span>
          </div>
        </div>
        {needsLink ? (
          <div className="row">
            <label htmlFor="gen-support" className="rl">
              Ссылка
            </label>
            <div className="rc">
              <input
                id="gen-support"
                className="field"
                maxLength={300}
                value={work.support}
                placeholder="@support_chat или https://t.me/support_bot"
                aria-invalid={supportBad}
                aria-describedby="gen-support-hint"
                onChange={(e) => edit({ support: e.target.value })}
                disabled={busy}
              />
              <span id="gen-support-hint" className={supportBad ? 'err' : 'cap'}>
                {supportBad
                  ? work.support.trim() === ''
                    ? 'Укажите чат, бота или сайт поддержки'
                    : 'Подойдёт @имя, ссылка t.me/… или адрес сайта с https://'
                  : supportUrl && supportUrl !== work.support.trim()
                    ? `Сохранится как ${supportUrl}`
                    : 'Чат или бот в Telegram, можно страница на сайте'}
              </span>
            </div>
          </div>
        ) : null}
      </section>

      <section className="sec" aria-labelledby="seo-h">
        <div className="sec-head">
          <h2 id="seo-h" className="section-title">
            Сайт в поиске и в превью ссылок
          </h2>
          <span className="muted">Так главную страницу покажут Яндекс и Google, а ссылку на сайт — Telegram и другие мессенджеры</span>
        </div>
        <div className="row">
          <label htmlFor="seo-title" className="rl">
            Заголовок
          </label>
          <div className="rc">
            <input
              id="seo-title"
              className="field"
              maxLength={limits.title + 30}
              value={work.title}
              placeholder={work.brand.trim() || core.brand_name}
              aria-invalid={titleBad}
              aria-describedby="seo-title-hint"
              onChange={(e) => edit({ title: e.target.value })}
              disabled={busy}
            />
            <span id="seo-title-hint" className="cap">
              <span className={titleBad ? 'seo-count over' : 'seo-count'}>
                {titleLen} из {limits.title}
              </span>
              {' · '}Пусто — будет название сервиса. В поиске видно около 60 символов
            </span>
          </div>
        </div>
        <div className="row">
          <label htmlFor="seo-desc" className="rl">
            Описание
          </label>
          <div className="rc">
            <textarea
              id="seo-desc"
              className="field"
              rows={3}
              maxLength={limits.desc + 60}
              value={work.desc}
              placeholder="Одно-два предложения о сервисе"
              aria-invalid={descBad}
              aria-describedby="seo-desc-hint"
              onChange={(e) => edit({ desc: e.target.value })}
              disabled={busy}
            />
            <span id="seo-desc-hint" className="cap">
              <span className={descBad ? 'seo-count over' : 'seo-count'}>
                {descLen} из {limits.desc}
              </span>
              {' · '}В поиске видно около 160 символов, в Telegram — первые две-три строки
            </span>
          </div>
        </div>
        <div className="row">
          <span className="rl">Поиск</span>
          <div className="rc">
            <span className="gen-line" style={{ minHeight: 40 }}>
              <Switch
                on={work.indexing}
                onToggle={() => edit({ indexing: !work.indexing })}
                label="Показывать главную страницу в поиске"
                describedBy="seo-index-hint"
                disabled={busy}
              />
              <span>Показывать главную страницу в поиске</span>
            </span>
            <span id="seo-index-hint" className="cap">
              Кабинет, мини-приложение и админка в поиск не попадают никогда
            </span>
          </div>
        </div>
        <div className="row">
          <span className="rl">Как это выглядит</span>
          <div className="rc">
            <div className="seo-preview" aria-hidden="true">
              <BrandMark name={work.brand.trim() || core.brand_name} logo={me.logo_url} className="mark" />
              <span className="seo-text">
                <span className="seo-host">{hostOf(core.public_url ?? window.location.origin)}</span>
                <span className="seo-title">{work.title.trim() || work.brand.trim() || core.brand_name}</span>
                <span className={work.desc.trim() ? 'seo-desc' : 'seo-desc empty'}>{work.desc.trim() || 'Описание не задано'}</span>
              </span>
            </div>
            <span className="cap">Картинка в превью — логотип из раздела «Оформление»</span>
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="inv-h">
        <div className="sec-head">
          <h2 id="inv-h" className="section-title">
            Счета и пополнение
          </h2>
          <span className="muted">Суммы в валюте сервиса, {currency === 'RUB' ? 'рублях' : 'долларах'}</span>
        </div>
        <div className="row">
          <label htmlFor="gen-ttl" className="rl">
            Счёт действует
          </label>
          <div className="rc">
            <span className="gen-line">
              <input id="gen-ttl" className="fld num" inputMode="numeric" value={work.ttl} aria-invalid={ttlBad} onChange={(e) => edit({ ttl: e.target.value.replace(/\D/g, '').slice(0, 4) })} disabled={busy} />
              <span className="muted">минут</span>
            </span>
            <span className={ttlBad ? 'err' : 'cap'}>{ttlBad ? 'От 5 минут до суток (1440)' : 'Потом неоплаченный счёт закрывается'}</span>
          </div>
        </div>
        <div className="row">
          <span className="rl">Пополнение баланса</span>
          <div className="rc">
            <span className="gen-line" style={{ flexWrap: 'wrap' }}>
              <span className="muted">от</span>
              <input className="fld num" style={{ width: 96 }} inputMode="decimal" value={work.min} aria-label="Минимальная сумма пополнения" aria-invalid={topupBad} onChange={(e) => edit({ min: e.target.value })} disabled={busy} />
              <span className="muted">до</span>
              <input className="fld num" style={{ width: 110 }} inputMode="decimal" value={work.max} aria-label="Максимальная сумма пополнения" aria-invalid={topupBad} onChange={(e) => edit({ max: e.target.value })} disabled={busy} />
              <span className="muted">{unit}</span>
            </span>
            {topupBad ? <span className="err">Минимум больше нуля и не больше максимума</span> : null}
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="tz-h">
        <div className="sec-head">
          <h2 id="tz-h" className="section-title">
            Часовой пояс Remnawave
          </h2>
          <span className="muted">По нему считаются сутки в статистике трафика и подключений, которые отдаёт панель, и дни в «Аналитике»</span>
        </div>
        <div className="row">
          <label htmlFor="gen-tz" className="rl">
            Пояс панели
          </label>
          <div className="rc">
            <select id="gen-tz" className="field" value={work.offset} onChange={(e) => edit({ offset: Number(e.target.value) })} disabled={busy} style={{ maxWidth: 200 }}>
              {OFFSETS.map((m) => (
                <option key={m} value={m}>
                  {offsetLabel(m)}
                </option>
              ))}
            </select>
          </div>
        </div>
      </section>

      <ProxySection
        view={proxyView}
        form={proxy}
        onChange={(next) => {
          setProxy(next);
          setStatus(null);
        }}
        onReset={resetProxy}
        busy={busy}
      />

      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button
          type="button"
          className="btn-line hov"
          disabled={busy}
          onClick={() => {
            setWork(base);
            setProxy(proxyForm(proxyView));
            setMaint(maintBase);
            setStatus(null);
          }}
        >
          Отменить
        </button>
        <span role="status" className={status?.bad ? 'bad' : undefined}>
          {status?.text ?? (dirty ? 'Есть несохранённые изменения' : '')}
        </span>
      </div>
    </SettingsLayout>
  );
}
