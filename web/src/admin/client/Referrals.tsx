import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { dateLong, money } from '../../lib/format';
import { Avatar } from '../Avatar';
import { useSession } from '../session';
import {
  askSetText,
  candidateLine,
  emptyText,
  FIRST_DESK,
  FIRST_PHONE,
  hidden,
  invitedLine,
  inviterLine,
  inviterWarning,
  leadText,
  MORE_PAGE,
  moreText,
  personContact,
  personTitle,
  SEARCH_MIN,
  type Candidate,
  type Fmt,
  type InvitedPage,
  type InvitedRow,
  type ReferralPerson,
  type ReferralSection,
} from './referralModel';

type Props = { clientId: number; phone?: boolean; onChanged?: () => void };

const FMT: Fmt = { money: (value) => money(value as Parameters<typeof money>[0]), date: (iso) => dateLong(iso) };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function searchable(query: string): boolean {
  const q = query.trim();
  return /^\d+$/.test(q) ? q.length > 0 : q.length >= SEARCH_MIN;
}

export function ClientReferrals({ clientId, phone = false, onChanged }: Props) {
  const { can } = useSession();
  const writable = can('clients_write');
  const base = `/api/admin/clients/${clientId}`;
  const [section, setSection] = useState<ReferralSection | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [moreBusy, setMoreBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const [query, setQuery] = useState('');
  const [found, setFound] = useState<Candidate[] | null>(null);
  const [chosen, setChosen] = useState<Candidate | null>(null);
  const [asking, setAsking] = useState<'remove' | number | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await api<ReferralSection>(`${base}/referrals?limit=${phone ? FIRST_PHONE : FIRST_DESK}`);
      setSection(data);
      setLoadError(null);
    } catch (err) {
      setLoadError(failText(err, 'Рефералы не загрузились'));
    }
  }, [base, phone]);

  useEffect(() => {
    setSection(null);
    setPicking(false);
    setQuery('');
    setFound(null);
    setChosen(null);
    setAsking(null);
    setError(null);
    void load();
  }, [load]);

  useEffect(() => {
    if (!picking || !searchable(query)) {
      setFound(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      api<Candidate[]>(`${base}/referrer/candidates?q=${encodeURIComponent(query.trim())}`)
        .then((list) => {
          if (alive) {
            setFound(list);
          }
        })
        .catch((err) => {
          if (alive) {
            setFound([]);
            setError(failText(err, 'Поиск не сработал, попробуйте ещё раз'));
          }
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [base, picking, query]);

  const closePick = () => {
    setPicking(false);
    setQuery('');
    setFound(null);
    setChosen(null);
  };

  const act = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      closePick();
      setAsking(null);
      await load();
      onChanged?.();
    } catch (err) {
      setError(failText(err, 'Не получилось, попробуйте ещё раз'));
      await load();
    } finally {
      setBusy(false);
    }
  };

  const more = async () => {
    if (!section?.invited.next) {
      return;
    }
    setMoreBusy(true);
    try {
      const page = await api<InvitedPage>(`${base}/referrals/invited?limit=${MORE_PAGE}&after=${section.invited.next}`);
      setSection((prev) => {
        if (!prev) {
          return prev;
        }
        const seen = new Set(prev.invited.items.map((item) => item.client.client_id));
        const fresh = page.items.filter((item) => !seen.has(item.client.client_id));
        return { ...prev, invited: { items: [...prev.invited.items, ...fresh], next: page.next } };
      });
    } catch (err) {
      setError(failText(err, 'Список не загрузился, попробуйте ещё раз'));
    } finally {
      setMoreBusy(false);
    }
  };

  if (loadError && !section) {
    return phone ? (
      <section className="pc-group" aria-label="Рефералы">
        <h2 className="hp-h2">Рефералы</h2>
        <p className="err pc-error" role="alert">
          {loadError}
        </p>
      </section>
    ) : (
      <section className="cc-sec cc-list cc-rf" aria-label="Рефералы">
        <h2 className="cc-h2">Рефералы</h2>
        <p className="cc-rf-lead err" role="alert">
          {loadError}
        </p>
      </section>
    );
  }
  if (!section || hidden(section)) {
    return null;
  }

  const inviter = section.inviter;
  const warning = inviter ? inviterWarning(inviter) : null;
  const empty = emptyText(section);
  const shown = section.invited.items.length;
  const moreLabel = moreText(section, shown);
  const ask = chosen ? askSetText(chosen, !!inviter) : null;

  const who = (person: ReferralPerson, line: string, small: string) => {
    const contact = personContact(person);
    return (
      <span className={phone ? 'pc-rf-who' : 'cc-rf-who'}>
        <span>
          <Link to={`/admin/clients/${person.client_id}`} className={phone ? 'pc-rf-name' : 'cc-rf-name'}>
            {personTitle(person)}
          </Link>
          {contact && !phone ? <span className="muted"> {contact}</span> : null}
        </span>
        <span className={small}>{line}</span>
      </span>
    );
  };

  const small = phone ? 'pc-small pc-muted' : 'cc-small muted';
  const ava = phone ? 'pc-rf-ava' : 'cc-rf-ava';

  const confirm = (head: string, tail: string, yes: string, run: () => void, cancel: () => void) =>
    phone ? (
      <div role="status" className="pc-ask pc-rf-ask">
        <span>
          <b>{head}</b>
          {tail}
        </span>
        <span className="pc-ask-btns">
          <button type="button" className="pc-ok press" disabled={busy} onClick={run}>
            {yes}
          </button>
          <button type="button" className="pc-cancel press" disabled={busy} onClick={cancel}>
            Отменить
          </button>
        </span>
      </div>
    ) : (
      <div role="status" className="cc-note cc-rf-ask">
        <span className="cc-note-text wide">
          <strong>{head}</strong>
          {tail}
        </span>
        <span className="cc-btns">
          <button type="button" className="b34 on-surface" disabled={busy} onClick={run}>
            {yes}
          </button>
          <button type="button" className="b34 on-surface" disabled={busy} onClick={cancel}>
            Отменить
          </button>
        </span>
      </div>
    );

  const pick = picking ? (
    <div className={phone ? 'pc-rf-pick' : 'cc-rf-pick'}>
      <input
        className="field"
        autoFocus
        value={query}
        maxLength={200}
        aria-label="Новый пригласивший"
        placeholder={phone ? 'Имя, @ник, почта или номер' : 'Имя, @ник, почта или номер клиента'}
        onChange={(event) => {
          setQuery(event.target.value);
          setChosen(null);
        }}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            closePick();
          }
        }}
      />
      {found && found.length === 0 ? <p className={`${small} cc-rf-hint`}>Никого не нашли</p> : null}
      {found && found.length > 0 ? (
        <div className="cc-rf-found">
          {found.map((candidate) => (
            <button
              key={candidate.client.client_id}
              type="button"
              className="cc-rf-opt"
              aria-pressed={chosen?.client.client_id === candidate.client.client_id}
              disabled={candidate.refusal !== null || busy}
              onClick={() => setChosen(candidate)}
            >
              <Avatar id={candidate.client.client_id} name={personTitle(candidate.client)} className="cc-rf-ava small" />
              <span className="cc-rf-who">
                <span>
                  <span className="cc-rf-name">{personTitle(candidate.client)}</span>
                  {personContact(candidate.client) ? <span className="muted"> {personContact(candidate.client)}</span> : null}
                </span>
                <span className={small}>{candidateLine(candidate, FMT)}</span>
              </span>
            </button>
          ))}
        </div>
      ) : null}
      {chosen && ask
        ? confirm(ask.head, ask.tail, 'Назначить', () => void act(() => api(`${base}/referrer`, { method: 'PUT', body: { referrer_id: chosen.client.client_id } })), () => setChosen(null))
        : null}
      {!chosen ? (
        <span className="cc-btns">
          <button type="button" className={phone ? 'pc-text press' : 'b34 hov'} onClick={closePick}>
            Отменить
          </button>
        </span>
      ) : null}
    </div>
  ) : null;

  const inviterBlock = inviter ? (
    <>
      <div className={phone ? 'pc-rf-row' : 'cc-rf-row'}>
        <Avatar id={inviter.client.client_id} name={personTitle(inviter.client)} className={ava} />
        {who(inviter.client, inviterLine(inviter, FMT), small)}
        {writable && !phone && !picking && asking !== 'remove' ? (
          <span className="cc-btns">
            <button type="button" className="b34 hov" onClick={() => setPicking(true)}>
              Сменить
            </button>
            <button type="button" className="b34 hov" onClick={() => setAsking('remove')}>
              Убрать
            </button>
          </span>
        ) : null}
      </div>
      {writable && phone && !picking && asking !== 'remove' ? (
        <span className="pc-rf-btns">
          <button type="button" className="pc-text press" onClick={() => setPicking(true)}>
            Сменить
          </button>
          <button type="button" className="pc-text press" onClick={() => setAsking('remove')}>
            Убрать
          </button>
        </span>
      ) : null}
      {warning ? <p className={`${small} cc-rf-warn`}>{warning}</p> : null}
      {asking === 'remove'
        ? confirm(
            'Убрать пригласившего?',
            ' Бонусы с новых оплат клиента никому не будут начисляться, уже начисленные не пересчитываются.',
            'Убрать',
            () => void act(() => api(`${base}/referrer`, { method: 'DELETE' })),
            () => setAsking(null),
          )
        : null}
    </>
  ) : (
    <div className={phone ? 'pc-rf-none' : 'cc-rf-none'}>
      <span className={phone ? 'pc-muted' : 'muted'}>Пригласившего нет</span>
      {writable && !picking ? (
        <button type="button" className={phone ? 'pc-text press' : 'b34 hov'} onClick={() => setPicking(true)}>
          Назначить пригласившего
        </button>
      ) : null}
    </div>
  );

  const row = (item: InvitedRow) => {
    const id = item.client.client_id;
    const bonus = item.bonus;
    return (
      <div key={id}>
        <div className={phone ? 'pc-rf-row' : 'cc-rf-row'}>
          <Avatar id={id} name={personTitle(item.client)} className={ava} />
          {who(item.client, phone && bonus ? `${invitedLine(item, FMT)} · бонусы ${FMT.money(bonus)}` : invitedLine(item, FMT), small)}
          {!phone && bonus ? <span className={bonus.minor > 0 ? 'cc-rf-sum num' : 'cc-rf-sum num zero'}>{FMT.money(bonus)}</span> : null}
          {writable && asking !== id ? (
            <button type="button" className={phone ? 'pc-text press' : 'b34 hov'} aria-label={`Отвязать: ${personTitle(item.client)}`} onClick={() => setAsking(id)}>
              Отвязать
            </button>
          ) : null}
        </div>
        {asking === id
          ? confirm(
              'Убрать из приглашённых?',
              ' Бонусы с новых оплат этого клиента начисляться не будут, уже начисленные останутся.',
              'Отвязать',
              () => void act(() => api(`${base}/referrals/invited/${id}`, { method: 'DELETE' })),
              () => setAsking(null),
            )
          : null}
      </div>
    );
  };

  const capClass = phone ? 'pc-rf-cap' : 'cc-rf-cap';
  const body = (
    <>
      {!section.module_enabled ? <p className={`${small} cc-rf-warn`}>Модуль «Рефералы» выключен: бонусы сейчас не начисляются.</p> : null}
      {empty && !picking ? (
        <div className={phone ? 'pc-rf-none' : 'cc-rf-none'}>
          <span className={phone ? 'pc-muted' : 'muted'}>{empty}</span>
          {writable ? (
            <button type="button" className={phone ? 'pc-text press' : 'b34 hov'} onClick={() => setPicking(true)}>
              Назначить пригласившего
            </button>
          ) : null}
        </div>
      ) : (
        <>
          <p className={phone ? 'pc-rf-lead' : 'cc-rf-lead'}>{leadText(section, FMT)}</p>
          <h3 className={capClass}>Пригласивший</h3>
          {inviterBlock}
        </>
      )}
      {pick}
      {error ? (
        <p className="err cc-rf-err" role="alert">
          {error}
        </p>
      ) : null}
      {section.invited_count > 0 ? (
        <>
          <h3 className={capClass}>
            <span>Приглашённые · {section.invited_count}</span>
            {section.money && !phone ? <span>бонусы с их оплат</span> : null}
          </h3>
          {section.invited.items.map(row)}
          {moreLabel ? (
            <button type="button" className={phone ? 'pc-morehist press' : 'cc-more cc-rf-more'} disabled={moreBusy} onClick={() => void more()}>
              {moreBusy ? 'Загружаем…' : moreLabel}
            </button>
          ) : null}
        </>
      ) : null}
    </>
  );

  if (phone) {
    return (
      <section className="pc-group" aria-labelledby="pc-rf-h">
        <h2 id="pc-rf-h" className="hp-h2">
          Рефералы
        </h2>
        <div className="pc-card pc-list pc-rf">{body}</div>
      </section>
    );
  }
  return (
    <section className="cc-sec cc-list cc-rf" aria-labelledby="cc-rf-h">
      <h2 id="cc-rf-h" className="cc-h2">
        Рефералы
      </h2>
      {body}
    </section>
  );
}
