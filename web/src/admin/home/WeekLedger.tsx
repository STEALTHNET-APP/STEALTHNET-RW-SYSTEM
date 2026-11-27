import { Link } from 'react-router';
import { groupDigits, money, NBSP, type Money } from '../../lib/format';
import { useSession } from '../session';
import { changePercent, dayLabel, lastWeek, type DayStat, type WeekRow } from './weekStats';

const TITLE = 'Последние 7 дней';
const TOTAL = 'За 7 дней';
const VERSUS = 'к прошлым 7 дням';
const WEEK_ANALYTICS = '/admin/m/analytics?period=7';

function Change({ value }: { value: number | null }) {
  if (value === null) {
    return null;
  }
  return (
    <>
      <span style={{ color: value >= 0 ? 'var(--ok)' : 'var(--bad)', fontWeight: 600 }}>
        {value >= 0 ? '+' : '−'}
        {Math.abs(value)}%
      </span>{' '}
      {VERSUS}
    </>
  );
}

function useAnalytics(): boolean {
  const { can, me } = useSession();
  return can('money') && me.menu.some((m) => m.module === 'analytics');
}

function cell(row: WeekRow): string {
  return row.back === 1 ? 'hm-ld-y' : '';
}

export function WeekLedger({ days, before, card }: { days: DayStat[]; before: Money | null | undefined; card?: boolean }) {
  const week = lastWeek(days);
  const analytics = useAnalytics();
  if (!week) {
    return null;
  }
  const sum = week.totals;
  const sees = sum.revenue !== null;
  const change = changePercent(sum.revenue, before);
  const line = (title: string, value: (s: DayStat) => string, total: string, sub = false) => (
    <tr className={sub ? 'hm-ld-sub' : undefined}>
      <th scope="row">{title}</th>
      {week.rows.map((row) => (
        <td key={row.day} className={`num ${cell(row)}`}>
          {value(row.stat)}
        </td>
      ))}
      <td className="num hm-ld-sum">{total}</td>
    </tr>
  );
  const body = (
    <div className="hm-week">
      <div className="hm-week-head">
        <h2 id="week-h" className="hm-cap">
          {TITLE}
        </h2>
        <span className="muted hm-small">
          <Change value={change} />
        </span>
        {analytics ? (
          <Link to={WEEK_ANALYTICS} className="hm-link hm-small hm-week-more">
            Подробнее в аналитике
          </Link>
        ) : null}
      </div>
      <div className="hm-ledger-wrap">
        <table className="hm-ledger" aria-labelledby="week-h">
          <thead>
            <tr>
              <th scope="col">
                <span className="sr-only">Показатель</span>
              </th>
              {week.rows.map((row) => (
                <th key={row.day} scope="col" className={cell(row)}>
                  {dayLabel(row.day, row.back)}
                </th>
              ))}
              <th scope="col" className="hm-ld-sum">
                {TOTAL}
              </th>
            </tr>
          </thead>
          <tbody>
            {sees ? line('Заработали', (s) => money(s.revenue, true), money(sum.revenue, true)) : null}
            {sees ? line('Оплат', (s) => groupDigits(s.payments ?? 0), groupDigits(sum.payments ?? 0)) : null}
            {line('Новые клиенты', (s) => groupDigits(s.new_clients), groupDigits(sum.newClients))}
            {sum.newPaid !== null ? line('из них с оплатой', (s) => groupDigits(s.new_clients_paid ?? 0), groupDigits(sum.newPaid), true) : null}
          </tbody>
        </table>
      </div>
    </div>
  );
  return card ? (
    <section aria-labelledby="week-h" className="hm-sec hm-weekcard">
      {body}
    </section>
  ) : (
    body
  );
}

export function PhoneWeek({ days, before }: { days: DayStat[]; before: Money | null | undefined }) {
  const week = lastWeek(days);
  if (!week) {
    return null;
  }
  const sum = week.totals;
  const sees = sum.revenue !== null;
  const paid = sum.newPaid !== null;
  const change = changePercent(sum.revenue, before);
  const clients = (n: number, p: number | null) => (
    <>
      {groupDigits(n)}
      {paid && p !== null ? <span className="muted">{`${NBSP}·${NBSP}${groupDigits(p)}`}</span> : null}
    </>
  );
  const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);
  return (
    <section aria-labelledby="hp-week" className="hp-group">
      <h2 id="hp-week" className="hp-h2">
        {TITLE}
      </h2>
      <div className="hp-card hp-week">
        <div className={sees ? 'hp-wrow head' : 'hp-wrow head short'}>
          <span>День</span>
          {sees ? <span className="hp-wsum">Заработали</span> : null}
          <span className="hp-wnew">{paid ? `Новые${NBSP}·${NBSP}с${NBSP}оплатой` : 'Новые'}</span>
        </div>
        {[...week.rows].reverse().map((row) => (
          <div key={row.day} className={`${sees ? 'hp-wrow' : 'hp-wrow short'}${row.back === 1 ? ' y' : ''}`}>
            <span>{capital(dayLabel(row.day, row.back))}</span>
            {sees ? <span className="hp-wsum num">{money(row.stat.revenue, true)}</span> : null}
            <span className="hp-wnew num">{clients(row.stat.new_clients, row.stat.new_clients_paid)}</span>
          </div>
        ))}
        <div className={sees ? 'hp-wrow total' : 'hp-wrow total short'}>
          <span>{TOTAL}</span>
          {sees ? <span className="hp-wsum num">{money(sum.revenue, true)}</span> : null}
          <span className="hp-wnew num">{clients(sum.newClients, sum.newPaid)}</span>
        </div>
        {change !== null ? (
          <p className="hp-wnote">
            <Change value={change} />
          </p>
        ) : null}
      </div>
    </section>
  );
}
