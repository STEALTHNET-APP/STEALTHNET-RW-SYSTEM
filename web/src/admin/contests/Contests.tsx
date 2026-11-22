import { useCallback, useEffect, useState } from 'react';
import { Link, Route, Routes, useNavigate, useParams } from 'react-router';
import { api } from '../../lib/api';
import { count } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import { ContestForm } from './ContestForm';
import { ContestPage } from './ContestPage';
import {
  BASE,
  disabledModule,
  draftOf,
  emptyDraft,
  failText,
  FILTERS,
  period,
  ROOT,
  statusTone,
  type Contest,
  type ListFilter,
  type ListItem,
  type Options,
  type Page,
} from './model';
import '../form.css';
import '../home/phone.css';
import '../promo/promo.css';
import './contests.css';

export function Contests() {
  return (
    <Routes>
      <Route index element={<List />} />
      <Route path="new" element={<Editor />} />
      <Route path=":id" element={<ContestPage />} />
      <Route path=":id/edit" element={<Editor />} />
    </Routes>
  );
}

export function useOptions() {
  const [options, setOptions] = useState<Options | null>(null);
  useEffect(() => {
    let alive = true;
    api<Options>(`${BASE}/options`)
      .then((o) => alive && setOptions(o))
      .catch(() => alive && setOptions(null));
    return () => {
      alive = false;
    };
  }, []);
  return options;
}

function NoAccess({ phone }: { phone: boolean }) {
  return phone ? (
    <div className="hp">
      <PhoneHeader title="Конкурсы" />
      <p className="hp-quiet hp-card">Раздел доступен администраторам с правом «Маркетинг».</p>
    </div>
  ) : (
    <>
      <h1 className="page-title">Конкурсы</h1>
      <section className="card card-pad">
        <p className="muted">Раздел доступен администраторам с правом «Маркетинг».</p>
      </section>
    </>
  );
}

function entriesText(item: ListItem): string {
  return item.counts.entries ? count(item.counts.entries, 'участник', 'участника', 'участников') : 'Пока никого';
}

function prizeLine(item: ListItem): string {
  if (item.prize.kind === 'tiered') {
    return `Призы по местам · ${count(item.winners, 'победитель', 'победителя', 'победителей')}`;
  }
  return item.winners > 1 ? `${item.prize_label} × ${item.winners}` : item.prize_label;
}

function List() {
  const phone = usePhone();
  const { can } = useSession();
  const allowed = can('marketing');
  const [filter, setFilter] = useState<ListFilter>('all');
  const [items, setItems] = useState<ListItem[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [more, setMore] = useState(false);

  const query = useCallback(
    (after: number | null) => {
      const params = new URLSearchParams({ limit: '30', status: filter });
      if (after !== null) {
        params.set('after', String(after));
      }
      return api<Page<ListItem>>(`${BASE}/contests?${params.toString()}`);
    },
    [filter],
  );

  useEffect(() => {
    if (!allowed) {
      return;
    }
    let alive = true;
    setItems(null);
    query(null)
      .then((page) => {
        if (alive) {
          setItems(page.items);
          setNext(page.next);
          setError(null);
          setOff(false);
        }
      })
      .catch((err) => {
        if (alive) {
          setItems([]);
          setNext(null);
          setOff(disabledModule(err));
          setError(failText(err, 'Конкурсы не загрузились'));
        }
      });
    return () => {
      alive = false;
    };
  }, [allowed, query]);

  const loadMore = async () => {
    if (next === null) {
      return;
    }
    setMore(true);
    try {
      const page = await query(next);
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Не загрузилось'));
    } finally {
      setMore(false);
    }
  };

  if (!allowed) {
    return <NoAccess phone={phone} />;
  }

  const empty = items !== null && items.length === 0 && !error;
  const filters = (
    <div role="group" aria-label="Какие конкурсы показать" className={phone ? 'pf-psegs ct-pfilter' : 'segs'}>
      {FILTERS.map(([key, label]) => (
        <button key={key} type="button" aria-pressed={filter === key} onClick={() => setFilter(key)}>
          {label}
        </button>
      ))}
    </div>
  );
  const offNote = error ? (
    <p className={phone ? 'hp-quiet hp-card' : 'pr-pad'} role="alert">
      <span className="err">{error}</span>
      {off ? (
        <>
          {' '}
          <Link to="/admin/modules" className="pr-inline-link">
            Включить модуль
          </Link>
        </>
      ) : null}
    </p>
  ) : null;

  if (phone) {
    return (
      <div className="hp ct-phone">
        <PhoneHeader title="Конкурсы">
          {off ? null : (
            <Link to={`${ROOT}/new`} className="ph-circ press" aria-label="Новый конкурс">
              <span className="ph-dot">
                <Icon name="plus" size={20} />
              </span>
            </Link>
          )}
        </PhoneHeader>
        {filters}
        {offNote}
        {items === null ? <p className="hp-quiet">Загружаем конкурсы…</p> : null}
        {empty ? <p className="hp-quiet hp-card">{filter === 'all' ? 'Конкурсов пока нет. Нажмите «+», чтобы создать первый.' : 'Здесь пусто'}</p> : null}
        {items && items.length ? (
          <div className="hp-card ct-plist">
            {items.map((item) => (
              <Link key={item.id} to={`${ROOT}/${item.id}`} className="ct-pitem press">
                <span className="ct-pmain">
                  <b>{item.title}</b>
                  <span>{prizeLine(item)}</span>
                  <span className="ct-pmeta">
                    <span className={`ct-state ${statusTone(item.status)}`}>{item.status_title}</span> · {entriesText(item)}
                  </span>
                </span>
                <Icon name="chevron-right" size={16} />
              </Link>
            ))}
          </div>
        ) : null}
        {next !== null ? (
          <button type="button" className="pf-quiet press ct-more" onClick={() => void loadMore()} disabled={more}>
            {more ? 'Загружаем…' : 'Показать ещё'}
          </button>
        ) : null}
      </div>
    );
  }

  return (
    <div className="pr-page">
      <div className="pr-head">
        <h1 className="page-title tight">Конкурсы</h1>
        <span className="pr-head-acts">
          {off ? null : (
            <Link to={`${ROOT}/new`} className="btn-main btn pr-new-btn ct-new">
              Новый конкурс
            </Link>
          )}
        </span>
      </div>
      <section className="card pr-list ct-list" aria-labelledby="ct-h">
        <div className="pr-top">
          <h2 id="ct-h" className="section-title">
            Розыгрыши
          </h2>
          {filters}
        </div>
        {offNote}
        {items === null ? <p className="muted pr-pad">Загружаем конкурсы…</p> : null}
        {empty ? <p className="muted pr-pad">{filter === 'all' ? 'Конкурсов пока нет' : 'Здесь пусто'}</p> : null}
        {items && items.length ? (
          <div role="table" aria-label="Конкурсы" className="ct-table">
            <div role="row" className="ct-trow ct-th">
              <span role="columnheader">Конкурс</span>
              <span role="columnheader">Приз</span>
              <span role="columnheader" className="pr-right">
                Участники
              </span>
              <span role="columnheader">Состояние</span>
              <span role="columnheader">
                <span className="sr-only">Открыть</span>
              </span>
            </div>
            {items.map((item) => (
              <Link key={item.id} role="row" to={`${ROOT}/${item.id}`} className="ct-trow hov">
                <span role="cell" className="pr-cell">
                  <span className="pr-strong">{item.title}</span>
                  <span className="pr-sub">{period(item)}</span>
                </span>
                <span role="cell" className="pr-cell">
                  {prizeLine(item)}
                </span>
                <span role="cell" className="pr-cell pr-right num">
                  {item.counts.entries}
                </span>
                <span role="cell" className={`pr-cell ct-state ${statusTone(item.status)}`}>
                  {item.status_title}
                  {item.counts.failed ? <span className="pr-sub bad">не выдано призов: {item.counts.failed}</span> : null}
                </span>
                <span role="cell" className="pr-cell ct-go">
                  <Icon name="chevron-right" size={16} />
                </span>
              </Link>
            ))}
          </div>
        ) : null}
        {next !== null ? (
          <div className="pr-pad">
            <button type="button" className="b40 hov" onClick={() => void loadMore()} disabled={more}>
              {more ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>
    </div>
  );
}

function Editor() {
  const { id } = useParams();
  const navigate = useNavigate();
  const phone = usePhone();
  const { me, can } = useSession();
  const options = useOptions();
  const [contest, setContest] = useState<Contest | null>(null);
  const [error, setError] = useState<string | null>(null);
  const editing = id !== undefined;

  useEffect(() => {
    if (!editing) {
      return;
    }
    let alive = true;
    api<Contest>(`${BASE}/contests/${encodeURIComponent(id)}`)
      .then((c) => alive && setContest(c))
      .catch((err) => alive && setError(failText(err, 'Конкурс не загрузился')));
    return () => {
      alive = false;
    };
  }, [editing, id]);

  if (!can('marketing')) {
    return <NoAccess phone={phone} />;
  }

  const title = editing ? 'Изменить конкурс' : 'Новый конкурс';
  const back = editing ? `${ROOT}/${id}` : ROOT;
  const ready = !editing || contest !== null;
  const form = ready ? (
    <ContestForm
      phone={phone}
      currency={options?.currency ?? me.currency}
      options={options}
      initial={contest ? draftOf(contest) : emptyDraft()}
      contest={contest}
      onCancel={() => navigate(back)}
      onSaved={(saved) => navigate(`${ROOT}/${saved.id}`, { state: { saved: !!contest } })}
    />
  ) : null;
  const note = editing && contest && !contest.editable.texts ? 'Конкурс закончился или отменён — изменить его уже нельзя.' : null;

  if (phone) {
    return (
      <div className="hp ct-phone">
        <PhoneHeader title={title} back={{ to: back, label: 'Назад' }} />
        {error ? <p className="hp-quiet hp-card err">{error}</p> : null}
        {note ? <p className="hp-quiet hp-card">{note}</p> : <div className="hp-card ct-pform">{form}</div>}
      </div>
    );
  }

  return (
    <div className="pr-page ct-editor">
      <Link to={back} className="ct-back hov">
        <Icon name="chevron-left" size={16} />
        {editing ? 'К конкурсу' : 'Все конкурсы'}
      </Link>
      <h1 className="page-title tight">{title}</h1>
      {error ? <p className="err">{error}</p> : null}
      {note ? <p className="card card-pad muted">{note}</p> : <section className="card ct-form-card">{form}</section>}
    </div>
  );
}
