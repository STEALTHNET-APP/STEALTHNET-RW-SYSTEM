import { useEffect, useState } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router';
import { api } from '../../lib/api';
import { count, dateLong, groupDigits, plural, timeHM } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import {
  BASE,
  deliveryText,
  failText,
  groups,
  initials,
  personName,
  personSub,
  placeLabel,
  prizeEach,
  prizeStateText,
  prizeText,
  ROOT,
  shortHash,
  statusTone,
  when,
  type Contest,
  type Entry,
  type MessageView,
  type Page,
  type Prize,
  type Winner,
} from './model';
import { useOptions } from './Contests';

const TONES = ['#2B5BF0', '#7A5AF0', '#1F9D6B', '#C2410C', '#0E7490', '#B4235A', '#5F6773'];

function tone(id: number): string {
  return TONES[id % TONES.length] ?? '#5F6773';
}

function Avatar({ person, id }: { person: Winner['client']; id: number }) {
  return (
    <span className="ct-ava" style={{ background: tone(id) }} aria-hidden="true">
      {initials(person)}
    </span>
  );
}

function PrizeLines({ prize }: { prize: Prize }) {
  if (prize.kind !== 'tiered') {
    return null;
  }
  return (
    <ul className="ct-ul ct-prizes">
      {prize.tiers.map((tier) => (
        <li key={`${tier.from_place}-${tier.to_place}`}>
          <b>{placeLabel(tier.from_place, tier.to_place)}</b> — {prizeText(tier.prize)}
        </li>
      ))}
    </ul>
  );
}

function left(iso: string): string {
  const ms = new Date(iso).getTime() - Date.now();
  if (ms <= 0) {
    return 'вот-вот';
  }
  const hours = Math.floor(ms / 3_600_000);
  if (hours >= 48) {
    return `через ${count(Math.floor(hours / 24), 'день', 'дня', 'дней')}`;
  }
  if (hours >= 1) {
    return `через ${count(hours, 'час', 'часа', 'часов')}`;
  }
  return `через ${count(Math.max(1, Math.floor(ms / 60_000)), 'минуту', 'минуты', 'минут')}`;
}

function Summary({ c }: { c: Contest }) {
  const lead =
    c.status === 'finished'
      ? `Жеребьёвка прошла ${when(c.drawn_at)}. Призы выдаются автоматически.`
      : c.status === 'canceled'
        ? `Конкурс отменён ${when(c.canceled_at)}, жеребьёвки не будет.`
        : c.status === 'drawing'
          ? 'Приём участников закончился, жеребьёвка пройдёт в течение минуты.'
          : c.status === 'scheduled'
            ? `Приём участников начнётся ${when(c.starts_at)}.`
            : `Идёт приём участников. Итоги ${when(c.ends_at)} — ${left(c.ends_at)}.`;
  const stats: Array<[string, string]> = [[groupDigits(c.counts.entries), plural(c.counts.entries, 'участник', 'участника', 'участников')]];
  if (c.status === 'finished') {
    stats.push([String(c.winners_list.length), plural(c.winners_list.length, 'победитель', 'победителя', 'победителей')]);
    stats.push([`${c.counts.issued} из ${c.winners_list.length}`, 'призов выдано']);
  } else {
    stats.push([String(c.winners), plural(c.winners, 'победитель', 'победителя', 'победителей')]);
  }
  return (
    <>
      <p className="ct-lead-line">{lead}</p>
      <div className="ct-stats">
        {stats.map(([value, label]) => (
          <div key={label}>
            <b className="num">{value}</b>
            <span>{label}</span>
          </div>
        ))}
      </div>
    </>
  );
}

function Winners({ c, phone, onRetry, busy }: { c: Contest; phone: boolean; onRetry: (id: number) => void; busy: number | null }) {
  if (!c.winners_list.length) {
    return c.status === 'finished' ? <p className="ct-pad muted">В розыгрыше никого не оказалось — призы не выданы.</p> : null;
  }
  return (
    <div role="table" aria-label="Победители" className={phone ? 'ct-wlist phone' : 'ct-wlist'}>
      {phone ? null : (
        <div role="row" className="ct-wrow ct-th">
          <span role="columnheader">Победитель</span>
          <span role="columnheader">Выдача приза</span>
          <span role="columnheader">
            <span className="sr-only">Действия</span>
          </span>
        </div>
      )}
      {c.winners_list.map((w) => {
        const state = prizeStateText(w);
        return (
          <div role="row" key={w.client_id} className="ct-wrow">
            <span role="cell" className="ct-who">
              <Avatar person={w.client} id={w.client_id} />
              <span>
                <span className="ct-win-row">
                  {w.place ? <span className="ct-win-place">{placeLabel(w.place, w.place)}</span> : null}
                  <Link to={`/admin/clients/${w.client_id}`} className="ct-name hov">
                    {personName(w.client, w.client_id)}
                  </Link>
                </span>
                <span className="ct-sub">{personSub(w.client, w.client_id)}</span>
                {w.prize_label ? <span className="ct-win-prize">{w.prize_label}</span> : null}
              </span>
            </span>
            <span role="cell" className={`ct-prize ${state.tone}`}>
              {state.text}
              {w.prize_code ? (
                <span className="ct-sub">
                  Код <span className="mono">{w.prize_code}</span>
                </span>
              ) : null}
            </span>
            <span role="cell" className="ct-wact">
              {w.prize_state === 'failed' ? (
                <button type="button" className="b34 hov" disabled={busy !== null} onClick={() => onRetry(w.client_id)}>
                  {busy === w.client_id ? 'Ставим…' : 'Выдать снова'}
                </button>
              ) : null}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function Fairness({ c }: { c: Contest }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="ct-fair">
      <dl className="ct-dl">
        {c.seed ? (
          <>
            <dt>Зерно жеребьёвки</dt>
            <dd className="mono ct-hash">{groups(c.seed)}</dd>
          </>
        ) : null}
        <dt>{c.seed ? 'Отпечаток до старта' : 'Отпечаток зерна'}</dt>
        <dd className="mono ct-hash">{c.seed ? `${shortHash(c.seed_hash)} — совпадает с зерном` : groups(c.seed_hash)}</dd>
        {c.list_hash ? (
          <>
            <dt>Список участников</dt>
            <dd>
              {count(c.entries_drawn ?? 0, 'человек', 'человека', 'человек')} в розыгрыше, отпечаток <span className="mono">{shortHash(c.list_hash)}</span> ·{' '}
              <a className="ct-link" href={`${BASE}/contests/${c.id}/entries.csv`} download>
                Скачать список
              </a>
            </dd>
          </>
        ) : null}
        {c.counts.excluded ? (
          <>
            <dt>Не прошли проверку</dt>
            <dd>{count(c.counts.excluded, 'участник', 'участника', 'участников')}: условия не выполнялись в момент итогов или участие закрыл антибот</dd>
          </>
        ) : null}
      </dl>
      <button type="button" className="ct-how hov" aria-expanded={open} onClick={() => setOpen(!open)}>
        Как проверить честность
      </button>
      {open ? (
        <p className="cap ct-how-text">
          Зерно — 64 знака, которые бот выбирает случайно при создании конкурса. До итогов виден только его отпечаток sha256, поэтому зерно нельзя подменить после того, как
          стали известны участники. После итогов зерно открывается: для каждого участника считается sha256 от строки «зерно:номер клиента», побеждают самые маленькие
          значения. Отпечаток зерна — sha256 от самого зерна, отпечаток списка — sha256 от номеров участников по возрастанию через перевод строки.
        </p>
      ) : null}
    </div>
  );
}

function Journal({ c }: { c: Contest }) {
  if (!c.log.length) {
    return null;
  }
  return (
    <ol className="ct-log" aria-label="Журнал">
      {c.log.map((l) => (
        <li key={l.id}>
          <span className="ct-log-at">
            {dateLong(l.at)}, {timeHM(l.at)}
          </span>
          <span>{l.text}</span>
        </li>
      ))}
    </ol>
  );
}

function filled(text: string, c: Contest): string {
  return text
    .replace(/<[^>]+>/g, '')
    .replaceAll('{title}', c.title)
    .replaceAll('{prize}', c.prize_label)
    .replaceAll('{winners}', String(c.winners))
    .replaceAll('{date}', when(c.ends_at));
}

function Messages({ items, c }: { items: MessageView[]; c: Contest }) {
  return (
    <div className="ct-msgs">
      {items.map((m) => (
        <div key={m.id} className="ct-mrow">
          <b className="ct-mwhen">{m.send_at ? when(m.send_at) : 'После итогов'}</b>
          <span className="ct-mbody">
            <span>{m.audience_title}</span>
            <span className="ct-sub ct-mtext">{filled(m.text, c)}</span>
          </span>
          <span className={m.state === 'skipped' ? 'ct-mstate warn' : 'ct-mstate'}>{deliveryText(m)}</span>
        </div>
      ))}
    </div>
  );
}

function preview(c: Contest): { text: string[]; buttons: string[] } {
  const lines = [c.title];
  if (c.description) {
    lines.push(c.description);
  }
  if (c.prize.kind === 'tiered') {
    lines.push('🎁 Призы:');
    for (const tier of c.prize.tiers) {
      lines.push(`${placeLabel(tier.from_place, tier.to_place)} — ${prizeText(tier.prize)}`);
    }
    lines.push(`Победителей: ${c.winners}.`);
  } else {
    lines.push(`Приз: ${c.prize_label}. Победителей: ${c.winners}.`);
  }
  lines.push(`Итоги — ${when(c.ends_at)}.`);
  if (c.condition_lines.length) {
    lines.push('', 'Условия');
    for (const l of c.condition_lines) {
      lines.push(`• ${l.text}`);
    }
  } else {
    lines.push('Участвовать может любой клиент.');
  }
  lines.push('', `Участников: ${c.counts.entries}`);
  return { text: lines, buttons: c.status === 'running' || c.status === 'scheduled' ? ['Участвовать', 'В меню'] : ['В меню'] };
}

function Preview({ c }: { c: Contest }) {
  const p = preview(c);
  return (
    <div className="ct-preview">
      <p className="ct-plabel">Так видит клиент в боте</p>
      <div className="ct-bubble">
        {p.text.map((line, i) =>
          i === 0 ? (
            <b key={i} className="ct-btitle">
              {line}
            </b>
          ) : line ? (
            <span key={i}>{line}</span>
          ) : (
            <span key={i} className="ct-gap" />
          ),
        )}
      </div>
      <div className="ct-kb">
        {p.buttons.map((b) => (
          <span key={b}>{b}</span>
        ))}
      </div>
    </div>
  );
}

function Actions({ c, phone, onCancel, onDelete, busy }: { c: Contest; phone: boolean; onCancel: () => void; onDelete: () => void; busy: boolean }) {
  const [asking, setAsking] = useState<'cancel' | 'delete' | null>(null);
  const canCancel = c.status === 'scheduled' || c.status === 'running' || c.status === 'drawing';
  const canDelete = c.status === 'scheduled' || (c.status === 'canceled' && c.counts.entries === 0);
  if (!canCancel && !canDelete && !c.editable.texts) {
    return null;
  }
  return (
    <div className={phone ? 'ct-acts phone' : 'ct-acts'}>
      {c.editable.texts ? (
        <Link to={`${ROOT}/${c.id}/edit`} className={phone ? 'np-ok press ct-edit' : 'btn-main btn ct-edit'}>
          Изменить
        </Link>
      ) : null}
      {canCancel ? (
        <button type="button" className={phone ? 'np-cancel press' : 'btn-line hov'} onClick={() => setAsking('cancel')} disabled={busy}>
          Отменить конкурс
        </button>
      ) : null}
      {canDelete ? (
        <button type="button" className="ct-danger hov" onClick={() => setAsking('delete')} disabled={busy}>
          Удалить
        </button>
      ) : null}
      {asking ? (
        <div className="ct-ask" role="alertdialog" aria-label={asking === 'cancel' ? 'Отменить конкурс' : 'Удалить конкурс'}>
          <span>
            {asking === 'cancel' ? (
              <>
                <b>Отменить конкурс?</b> Жеребьёвки не будет, неотправленные сообщения не уйдут. Вернуть конкурс нельзя.
              </>
            ) : (
              <>
                <b>Удалить конкурс?</b> Он исчезнет из списка насовсем.
              </>
            )}
          </span>
          <span className="ct-ask-btns">
            <button
              type="button"
              className="b34 on-surface hov"
              disabled={busy}
              onClick={() => {
                setAsking(null);
                if (asking === 'cancel') {
                  onCancel();
                } else {
                  onDelete();
                }
              }}
            >
              {asking === 'cancel' ? 'Отменить конкурс' : 'Удалить'}
            </button>
            <button type="button" className="b34 on-surface hov" onClick={() => setAsking(null)} disabled={busy}>
              Не нужно
            </button>
          </span>
        </div>
      ) : null}
    </div>
  );
}

function Participants({ c }: { c: Contest }) {
  const [filter, setFilter] = useState<'all' | 'excluded'>('all');
  const [page, setPage] = useState<Page<Entry> | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api<Page<Entry>>(`${BASE}/contests/${c.id}/entries?limit=20&filter=${filter}`)
      .then((p) => alive && setPage(p))
      .catch((err) => alive && setError(failText(err, 'Участники не загрузились')));
    return () => {
      alive = false;
    };
  }, [c.id, c.counts.entries, filter]);
  if (!c.counts.entries) {
    return <p className="ct-pad muted">Участников пока нет. Клиенты вступают кнопкой «Участвовать» в боте.</p>;
  }
  return (
    <>
      {c.counts.excluded ? (
        <div className="ct-pad ct-seg">
          <div role="group" aria-label="Кого показать" className="segs">
            <button type="button" aria-pressed={filter === 'all'} onClick={() => setFilter('all')}>
              Все
            </button>
            <button type="button" aria-pressed={filter === 'excluded'} onClick={() => setFilter('excluded')}>
              Не прошли · {c.counts.excluded}
            </button>
          </div>
        </div>
      ) : null}
      {error ? <p className="ct-pad err">{error}</p> : null}
      <div className="ct-plist2">
        {(page?.items ?? []).map((e) => (
          <div key={e.client_id} className="ct-prow">
            <Avatar person={e.client} id={e.client_id} />
            <span className="ct-pinfo">
              <Link to={`/admin/clients/${e.client_id}`} className="ct-name hov">
                {personName(e.client, e.client_id)}
              </Link>
              <span className="ct-sub">{e.excluded ?? `участвует с ${dateLong(e.joined_at)}`}</span>
            </span>
            {e.place ? <span className="ct-cup">Победитель</span> : null}
          </div>
        ))}
      </div>
      {page && page.next !== null ? <p className="ct-pad cap">Показаны первые 20 — полный список в файле «Скачать список».</p> : null}
    </>
  );
}

export function ContestPage() {
  const { id = '' } = useParams();
  const phone = usePhone();
  const navigate = useNavigate();
  const location = useLocation();
  const { can } = useSession();
  const options = useOptions();
  const [c, setC] = useState<Contest | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [retrying, setRetrying] = useState<number | null>(null);
  const [toast, setToast] = useState<string | null>((location.state as { saved?: boolean } | null)?.saved ? 'Изменения сохранены' : null);

  useEffect(() => {
    let alive = true;
    const load = () =>
      api<Contest>(`${BASE}/contests/${encodeURIComponent(id)}`)
        .then((next) => {
          if (alive) {
            setC(next);
            setError(null);
          }
        })
        .catch((err) => alive && setError(failText(err, 'Конкурс не загрузился')));
    void load();
    const timer = window.setInterval(() => void load(), 20_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [id]);

  if (!can('marketing')) {
    return <p className="muted">Раздел доступен администраторам с правом «Маркетинг».</p>;
  }

  const act = async (path: string, done: string) => {
    if (!c) {
      return;
    }
    setBusy(true);
    try {
      const next = await api<Contest>(`${BASE}/contests/${c.id}/${path}`, { method: 'POST' });
      setC(next);
      setToast(done);
    } catch (err) {
      setToast(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!c) {
      return;
    }
    setBusy(true);
    try {
      await api(`${BASE}/contests/${c.id}`, { method: 'DELETE' });
      navigate(ROOT);
    } catch (err) {
      setToast(failText(err, 'Не удалось удалить'));
      setBusy(false);
    }
  };

  const retry = async (client: number) => {
    if (!c) {
      return;
    }
    setRetrying(client);
    try {
      setC(await api<Contest>(`${BASE}/contests/${c.id}/winners/${client}/retry`, { method: 'POST' }));
      setToast('Приз поставлен на выдачу — бот выдаст его в течение минуты');
    } catch (err) {
      setToast(failText(err, 'Не получилось'));
    } finally {
      setRetrying(null);
    }
  };

  if (error && !c) {
    return phone ? (
      <div className="hp">
        <PhoneHeader title="Конкурс" back={{ to: ROOT, label: 'Все конкурсы' }} />
        <p className="hp-quiet hp-card err">{error}</p>
      </div>
    ) : (
      <div className="pr-page">
        <Link to={ROOT} className="ct-back hov">
          <Icon name="chevron-left" size={16} />
          Все конкурсы
        </Link>
        <p className="err">{error}</p>
      </div>
    );
  }
  if (!c) {
    return <p className="muted ct-loading">Загружаем конкурс…</p>;
  }

  const broadcastOff = options && !options.broadcast_enabled && c.messages.some((m) => m.state === 'planned');
  const warn = broadcastOff ? (
    <p className="ct-warn" role="note">
      Модуль «Рассылки» выключен — напоминания и итоги не уйдут, пока его не включить. Победители всё равно получат личные сообщения о призе.
    </p>
  ) : null;
  const result = (
    <>
      <Summary c={c} />
      <Winners c={c} phone={phone} onRetry={(client) => void retry(client)} busy={retrying} />
    </>
  );

  if (phone) {
    return (
      <div className="hp ct-phone">
        <PhoneHeader title={c.title} back={{ to: ROOT, label: 'Все конкурсы' }} />
        <p className="np-sum">
          <span className={`ct-state ${statusTone(c.status)}`}>{c.status_title}</span>
        </p>
        {toast ? (
          <p className="hp-quiet hp-card" role="status">
            {toast}
          </p>
        ) : null}
        {warn}
        <section className="hp-group" aria-labelledby="ct-p-res">
          <h2 id="ct-p-res" className="hp-h2">
            {c.status === 'finished' ? 'Итог' : 'Ход конкурса'}
          </h2>
          <div className="hp-card ct-pcard">{result}</div>
        </section>
        <section className="hp-group" aria-labelledby="ct-p-rules">
          <h2 id="ct-p-rules" className="hp-h2">
            Приз и условия
          </h2>
          <div className="hp-card ct-pcard ct-rules">
            {c.prize.kind === 'tiered' ? <PrizeLines prize={c.prize} /> : <p>{prizeEach(c.prize, c.winners)}</p>}
            <p className="ct-sub">
              {when(c.starts_at)} — {when(c.ends_at)}
            </p>
            {c.condition_lines.length ? c.condition_lines.map((l) => <p key={l.key}>• {l.text}</p>) : <p>Без условий: участвовать может любой клиент</p>}
          </div>
        </section>
        <section className="hp-group" aria-labelledby="ct-p-msg">
          <h2 id="ct-p-msg" className="hp-h2">
            Сообщения
          </h2>
          <div className="hp-card ct-pcard">
            <Messages items={c.messages} c={c} />
          </div>
        </section>
        <section className="hp-group" aria-labelledby="ct-p-fair">
          <h2 id="ct-p-fair" className="hp-h2">
            Жеребьёвка и журнал
          </h2>
          <div className="hp-card ct-pcard">
            <Fairness c={c} />
            <Journal c={c} />
          </div>
        </section>
        <section className="hp-group" aria-labelledby="ct-p-who">
          <h2 id="ct-p-who" className="hp-h2">
            Участники
          </h2>
          <div className="hp-card ct-pcard">
            <Participants c={c} />
          </div>
        </section>
        <Actions c={c} phone onCancel={() => void act('cancel', 'Конкурс отменён')} onDelete={() => void remove()} busy={busy} />
      </div>
    );
  }

  return (
    <div className="pr-page ct-page">
      <Link to={ROOT} className="ct-back hov">
        <Icon name="chevron-left" size={16} />
        Все конкурсы
      </Link>
      <div className="pr-head">
        <h1 className="page-title tight">{c.title}</h1>
        <span className={`ct-state ct-head-state ${statusTone(c.status)}`}>{c.status_title}</span>
      </div>
      {toast ? (
        <p className="ct-toast" role="status">
          {toast}
        </p>
      ) : null}
      {warn}
      <div className="ct-cols">
        <div>
          <section className="card ct-card" aria-labelledby="ct-res">
            <h2 id="ct-res" className="section-title ct-title">
              {c.status === 'finished' ? 'Итог' : 'Ход конкурса'}
            </h2>
            {result}
            <Fairness c={c} />
            <Journal c={c} />
          </section>
          <section className="card ct-card" aria-labelledby="ct-rules">
            <h2 id="ct-rules" className="section-title ct-title">
              Приз и условия
            </h2>
            <dl className="ct-dl ct-dl-pad">
              <dt>Приз</dt>
              <dd>{c.prize.kind === 'tiered' ? <PrizeLines prize={c.prize} /> : prizeEach(c.prize, c.winners)}</dd>
              <dt>Сроки</dt>
              <dd>
                С {when(c.starts_at)} по {when(c.ends_at)}
              </dd>
              <dt>Условия</dt>
              <dd>
                {c.condition_lines.length ? (
                  <ul className="ct-ul">
                    {c.condition_lines.map((l) => (
                      <li key={l.key}>{l.text}</li>
                    ))}
                  </ul>
                ) : (
                  'Без условий: участвовать может любой клиент бота'
                )}
              </dd>
            </dl>
          </section>
          <section className="card ct-card" aria-labelledby="ct-msg">
            <h2 id="ct-msg" className="section-title ct-title">
              Сообщения
            </h2>
            <p className="ct-pad-lead cap">Уходят через рассылки в бот, с кнопкой конкурса. Победители получают личное сообщение с призом.</p>
            <Messages items={c.messages} c={c} />
          </section>
          <section className="card ct-card" aria-labelledby="ct-who">
            <h2 id="ct-who" className="section-title ct-title">
              Участники
            </h2>
            <Participants c={c} />
          </section>
          <Actions c={c} phone={false} onCancel={() => void act('cancel', 'Конкурс отменён')} onDelete={() => void remove()} busy={busy} />
        </div>
        <aside className="ct-side">
          <Preview c={c} />
        </aside>
      </div>
    </div>
  );
}
