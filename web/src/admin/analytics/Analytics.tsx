import { useEffect, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { NoAccess } from '../promo/Promo';
import { AnalyticsPhone } from './AnalyticsPhone';
import { downloadRows, metricRows, tabRows } from './csv';
import { MetricDetail } from './detail';
import { ListDrawer, type Opened } from './ListDrawer';
import {
  change,
  compareText,
  FILTERS,
  filterLabel,
  format,
  PRESETS,
  periodText,
  presetFromParam,
  TABS,
  useOptions,
  useQueryState,
  useSelected,
  useTab,
  type Compare,
  type FilterKey,
  type Metric,
  type Options,
  type Preset,
  type Query,
  type Tab,
  type TabReport,
} from './model';
import './analytics.css';

export type Page = {
  tab: string;
  setTab: (tab: string) => void;
  q: Query;
  setQ: (next: Partial<Query>) => void;
  data: Tab;
  options: Options | null;
  metric: Metric | null;
  select: (key: string) => void;
  opened: Opened | null;
  open: (o: Opened | null) => void;
};

export function Analytics() {
  const phone = usePhone();
  const [params] = useSearchParams();
  const wanted = params.get('tab');
  const [tab, setTab] = useState(TABS.some((t) => t[0] === wanted) ? (wanted as string) : 'money');
  const [q, setQ, syncToday] = useQueryState(params.get('source') ? { source: params.get('source') as string } : {}, presetFromParam(params.get('period')) ?? 'month');
  const data = useTab(tab, q);
  const panelToday = data.report?.range.today;
  useEffect(() => {
    if (panelToday) {
      syncToday(panelToday);
    }
  }, [panelToday, syncToday]);
  const options = useOptions(data.allowed && !data.off);
  const [metric, select] = useSelected(data.report);
  const [opened, open] = useState<Opened | null>(null);
  const page: Page = { tab, setTab, q, setQ, data, options, metric, select, opened, open };
  return phone ? <AnalyticsPhone page={page} /> : <AnalyticsDesk page={page} />;
}

export const COMPARES: Array<[Compare, string, string]> = [
  ['prev', 'с прошлым периодом', 'к прошлому'],
  ['year', 'с прошлым годом', 'к году назад'],
  ['none', 'не сравнивать', 'без сравнения'],
];

export function Off() {
  return (
    <section className="card card-pad an-off">
      <p>Модуль «Бизнес-аналитика» выключен.</p>
      <Link to="/admin/modules" className="b40 hov an-off-link">
        Открыть модули
      </Link>
    </section>
  );
}

export const PREPARING = 'Считаем аналитику в первый раз. На большой базе это займёт пару минут — страница обновится сама.';

export function comparison(m: Metric, report: TabReport): string | null {
  if (m.value === null || m.prev === null || m.prev === undefined || !report.range.prev) {
    return null;
  }
  const c = report.currency;
  const raw = compareText(report.range) ?? '';
  const when = `за ${raw.charAt(0).toLowerCase()}${raw.slice(1)}`;
  const diff = m.value - m.prev;
  if (m.unit === 'percent') {
    const d = Math.round(diff * 10) / 10;
    if (d === 0) {
      return `столько же, сколько ${when}`;
    }
    return `на ${String(Math.abs(d)).replace('.', ',')} п. ${d > 0 ? 'больше' : 'меньше'}, чем ${when}`;
  }
  if (diff === 0) {
    return `столько же, сколько ${when}`;
  }
  const ch = change(m.value, m.prev, m.unit);
  return `на ${format(Math.abs(diff), m.unit, c)} ${diff > 0 ? 'больше' : 'меньше'}, чем ${when}${ch ? ` (${ch})` : ''}`;
}

function useOutside(open: boolean, close: () => void) {
  const box = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) {
      return;
    }
    const click = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) {
        close();
      }
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        close();
      }
    };
    document.addEventListener('mousedown', click);
    document.addEventListener('keydown', key);
    return () => {
      document.removeEventListener('mousedown', click);
      document.removeEventListener('keydown', key);
    };
  }, [open, close]);
  return box;
}

export function FilterFields({ page, wide }: { page: Page; wide?: boolean }) {
  const { q, setQ, options } = page;
  return (
    <div className={wide ? 'ax-fields wide' : 'ax-fields'}>
      {FILTERS.map(([key, label, list]) => (
        <label key={key} className="ax-field">
          <span>{label}</span>
          <select
            value={q.filters[key] ?? ''}
            onChange={(e) => setQ({ filters: { ...q.filters, [key]: e.target.value || undefined } })}
          >
            <option value="">{key === 'source' ? 'все источники' : 'все'}</option>
            {(options?.[list] ?? []).map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </label>
      ))}
    </div>
  );
}

export function Chips({ page }: { page: Page }) {
  const { q, setQ, options } = page;
  const active = (Object.entries(q.filters) as Array<[FilterKey, string | undefined]>).filter(([, v]) => v);
  if (!active.length) {
    return null;
  }
  return (
    <div className="ax-chips">
      {active.map(([key, value]) => (
        <span key={key} className="ax-chip">
          <span>{FILTERS.find((f) => f[0] === key)?.[1]}:</span> {filterLabel(options, key, value as string)}
          <button type="button" aria-label="Убрать фильтр" onClick={() => setQ({ filters: { ...q.filters, [key]: undefined } })}>
            <Icon name="close" size={14} stroke={2.2} />
          </button>
        </span>
      ))}
      <button type="button" className="ax-reset" onClick={() => setQ({ filters: {} })}>
        Сбросить
      </button>
    </div>
  );
}

function FiltersButton({ page }: { page: Page }) {
  const [open, setOpen] = useState(false);
  const box = useOutside(open, () => setOpen(false));
  const count = Object.values(page.q.filters).filter(Boolean).length;
  return (
    <div className="an-csv" ref={box}>
      <button type="button" className="b40 hov ax-btn" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Icon name="sliders" size={17} stroke={2} />
        Фильтры
        {count ? <span className="ax-count">{count}</span> : null}
      </button>
      {open ? (
        <div className="an-csv-menu ax-filter-menu" role="dialog" aria-label="Фильтры">
          <FilterFields page={page} />
          <p className="ax-note muted">Фильтр, который к показателю не относится, не применяется — рядом с цифрой будет пометка.</p>
        </div>
      ) : null}
    </div>
  );
}

function CsvButton({ page }: { page: Page }) {
  const [open, setOpen] = useState(false);
  const box = useOutside(open, () => setOpen(false));
  const report = page.data.report;
  const metric = page.metric;
  if (!report) {
    return null;
  }
  const label = TABS.find((t) => t[0] === page.tab)?.[1] ?? 'Аналитика';
  return (
    <div className="an-csv" ref={box}>
      <button type="button" className="b40 hov an-csv-btn" aria-expanded={open} aria-haspopup="menu" onClick={() => setOpen(!open)}>
        <Icon name="arrow-down" size={17} stroke={2.1} />
        CSV
      </button>
      {open ? (
        <div className="an-csv-menu" role="menu">
          {metric ? (
            <button type="button" role="menuitem" className="hov" onClick={() => { downloadRows(`${metric.title} ${report.range.from} ${report.range.to}`, metricRows(metric, report)); setOpen(false); }}>
              {`«${metric.title}» подробно`}
            </button>
          ) : null}
          <button type="button" role="menuitem" className="hov" onClick={() => { downloadRows(`${label} ${report.range.from} ${report.range.to}`, tabRows(report)); setOpen(false); }}>
            {`Все показатели вкладки «${label}»`}
          </button>
        </div>
      ) : null}
    </div>
  );
}

export function PeriodSelect({ page, phone }: { page: Page; phone?: boolean }) {
  const { q, setQ } = page;
  const select = (
    <select value={q.preset} onChange={(e) => setQ({ preset: e.target.value as Preset })}>
      {PRESETS.map(([p, label]) => (
        <option key={p} value={p}>
          {p === 'custom' && q.preset === 'custom' ? periodText(q.from, q.to) : label}
        </option>
      ))}
    </select>
  );
  return phone ? (
    <label className="anp-chip">
      {select}
      <Icon name="chevron-down" size={16} stroke={2.2} />
    </label>
  ) : (
    <label className="an-select">
      <span>Период</span>
      {select}
      <Icon name="chevron-down" size={15} stroke={2.2} />
    </label>
  );
}

export function CustomDates({ page }: { page: Page }) {
  const { q, setQ } = page;
  if (q.preset !== 'custom') {
    return null;
  }
  return (
    <div className="ax-dates">
      <label>
        <span>с</span>
        <input type="date" className="field" value={q.from} max={q.to} onChange={(e) => e.target.value && setQ({ from: e.target.value, step: null })} />
      </label>
      <label>
        <span>по</span>
        <input type="date" className="field" value={q.to} min={q.from} onChange={(e) => e.target.value && setQ({ to: e.target.value, step: null })} />
      </label>
    </div>
  );
}

export function CompareSelect({ page, phone }: { page: Page; phone?: boolean }) {
  const { q, setQ } = page;
  const select = (
    <select value={q.compare} onChange={(e) => setQ({ compare: e.target.value as Compare })}>
      {COMPARES.map(([c, label, short]) => (
        <option key={c} value={c}>
          {phone ? short : label}
        </option>
      ))}
    </select>
  );
  return phone ? (
    <label className="anp-chip">
      {select}
      <Icon name="chevron-down" size={16} stroke={2.2} />
    </label>
  ) : (
    <label className="an-select">
      <span>Сравнить</span>
      {select}
      <Icon name="chevron-down" size={15} stroke={2.2} />
    </label>
  );
}

export function rowChange(m: Metric): string | null {
  return change(m.value, m.prev, m.unit);
}

function AnalyticsDesk({ page }: { page: Page }) {
  const { tab, setTab, data, metric, select } = page;
  const report = data.report;
  if (!data.allowed) {
    return <NoAccess title="Аналитика" text="Раздел доступен администраторам с правом «Деньги»." />;
  }
  return (
    <div className="an">
      <header className="an-head">
        <h1 className="page-title tight">Аналитика</h1>
        {data.off ? null : (
          <div className="an-tools">
            <PeriodSelect page={page} />
            <CompareSelect page={page} />
            <FiltersButton page={page} />
            <CsvButton page={page} />
          </div>
        )}
      </header>
      {data.off ? (
        <Off />
      ) : (
        <>
          <CustomDates page={page} />
          <Chips page={page} />
          <nav className="an-tabs" aria-label="Разделы аналитики">
            {TABS.map(([key, label]) => (
              <button key={key} type="button" className={tab === key ? 'an-tab on' : 'an-tab'} aria-current={tab === key ? 'page' : undefined} onClick={() => setTab(key)}>
                {label}
              </button>
            ))}
          </nav>
          {report?.notes?.map((n) => (
            <p key={n} className="ax-note muted ax-tabnote">
              {n}
            </p>
          ))}
          {data.error ? (
            <div className="card card-pad an-problem" role="alert">
              <span>{data.error}</span>
              <button type="button" className="b36 hov" onClick={data.reload}>
                Повторить
              </button>
            </div>
          ) : null}
          {report?.preparing ? (
            <section className="card card-pad">
              <p className="muted">{PREPARING}</p>
            </section>
          ) : report ? (
            <section className={data.loading ? 'card ax-pane busy' : 'card ax-pane'} aria-busy={data.loading}>
              <div className="ax-list" role="list">
                {report.groups.map((g, gi) => (
                  <div key={g.title ?? `g${gi}`} className="ax-group" role="presentation">
                    {g.title ? <div className="ax-cap">{g.title}</div> : null}
                    {g.metrics.map((m) => (
                      <button
                        key={m.key}
                        type="button"
                        role="listitem"
                        className={`ax-row${m.key === metric?.key ? ' on' : ''}${m.sub ? ' sub' : ''}`}
                        aria-current={m.key === metric?.key ? 'true' : undefined}
                        onClick={() => select(m.key)}
                      >
                        <span className="ax-name">
                          {m.title}
                          {m.note ? (
                            <span className="ax-rnote" title={m.note}>
                              {m.note}
                            </span>
                          ) : null}
                        </span>
                        <span className="ax-chg">{rowChange(m) ?? ''}</span>
                        <span className="ax-val num">{format(m.value, m.unit, report.currency, true)}</span>
                      </button>
                    ))}
                  </div>
                ))}
              </div>
              {metric ? <DeskDetail page={page} metric={metric} report={report} /> : null}
            </section>
          ) : !data.error ? (
            <p className="an-wait muted">Считаем…</p>
          ) : null}
        </>
      )}
      <ListDrawer opened={page.opened} q={page.q} currency={report?.currency ?? 'RUB'} onClose={() => page.open(null)} phone={false} />
    </div>
  );
}

function DeskDetail({ page, metric, report }: { page: Page; metric: Metric; report: TabReport }) {
  const sentence = comparison(metric, report);
  return (
    <div className="ax-detail">
      <div className="ax-dhead">
        <div>
          <h2 className="ax-h2">{metric.title}</h2>
          <span className="ax-sub">
            {periodText(report.range.from, report.range.to)}
            {metric.hint ? ` · ${metric.hint}` : ''}
          </span>
        </div>
      </div>
      {metric.detail.type !== 'cohorts' ? (
        <div className="ax-sum">
          <strong className="ax-big num">{format(metric.value, metric.unit, report.currency)}</strong>
          {sentence ? <span className="ax-vs">{sentence}</span> : metric.note ? <span className="ax-vs">{metric.note}</span> : null}
        </div>
      ) : null}
      <MetricDetail metric={metric} report={report} q={page.q} setQ={page.setQ} onList={(ref, title) => page.open({ ref, title })} />
    </div>
  );
}
