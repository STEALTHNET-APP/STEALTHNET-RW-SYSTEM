import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { useSession } from '../session';
import { ChannelPreview } from './Preview';
import {
  BASE,
  CHANNELS,
  ROOT,
  at,
  count,
  done,
  eta,
  failText,
  hasBot,
  hasEmail,
  outcome,
  percent,
  reachText,
  statusLine,
  type Broadcast,
  type Reach,
} from './model';

type Props = {
  broadcast: Broadcast;
  onChange: (broadcast: Broadcast) => void;
  onCopy: (broadcast: Broadcast) => void;
};

export function Report({ broadcast, onChange, onCopy }: Props) {
  const { me } = useSession();
  const b = broadcast;
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reach, setReach] = useState<Reach | null>(null);
  const live = b.status === 'sending' || b.status === 'scheduled';

  useEffect(() => {
    if (!live) {
      return;
    }
    const timer = window.setInterval(() => {
      api<Broadcast>(`${BASE}/broadcasts/${b.id}`)
        .then(onChange)
        .catch(() => undefined);
    }, 2000);
    return () => window.clearInterval(timer);
  }, [live, b.id, onChange]);

  const audienceKey = JSON.stringify(b.audience);
  useEffect(() => {
    if (b.status !== 'scheduled') {
      return;
    }
    api<Reach>(`${BASE}/audience`, { method: 'POST', body: { audience: JSON.parse(audienceKey) as Broadcast['audience'] } })
      .then(setReach)
      .catch(() => setReach(null));
  }, [b.status, audienceKey]);

  const act = async (path: 'cancel' | 'copy') => {
    setBusy(true);
    setError(null);
    try {
      const next = await api<Broadcast>(`${BASE}/broadcasts/${b.id}/${path}`, { method: 'POST' });
      setConfirm(false);
      if (path === 'copy') {
        onCopy(next);
      } else {
        onChange(next);
      }
    } catch (err) {
      setError(failText(err, path === 'copy' ? 'Копия не создалась' : 'Не получилось остановить'));
    } finally {
      setBusy(false);
    }
  };

  const status = statusLine(b);
  const p = b.progress;
  const mailFailed = b.channels.find((lane) => lane.channel === 'email')?.progress.failed ?? 0;
  const showProgress = p.total > 0 && b.status !== 'scheduled';

  return (
    <div className="bc-grid">
      <div className="bc-col">
        <section className="sec" aria-labelledby="bc-run-h">
          <div className="sec-head bc-head">
            <h2 id="bc-run-h" className="section-title">
              {b.status === 'scheduled' ? 'Отправка' : b.status === 'sending' ? 'Ход отправки' : 'Итог'}
            </h2>
            <span className={`bc-status ${status.tone}`}>
              {b.status === 'sending' && p.total > 0 && !b.queued_behind && !b.notice && !b.mail_notice ? eta(b.channels) : status.text}
            </span>
          </div>
          {showProgress ? (
            <div className="bc-progress">
              <span className="bc-bar wide" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent(p)}>
                <span style={{ width: `${percent(p)}%` }} />
              </span>
              <dl className="bc-dl">
                <div>
                  <dt>{b.channel === 'bot' ? 'Доставлено' : 'Отправлено'}</dt>
                  <dd className="num">{count(p.sent)}</dd>
                </div>
                <div>
                  <dt>Не доставлено</dt>
                  <dd className="num">{count(p.failed)}</dd>
                </div>
                {hasBot(b.channel) ? (
                  <div>
                    <dt>Бот остановлен</dt>
                    <dd className="num">{count(p.blocked)}</dd>
                  </div>
                ) : null}
                {b.status === 'sending' ? (
                  <div>
                    <dt>Осталось</dt>
                    <dd className="num">{count(p.pending)}</dd>
                  </div>
                ) : null}
                {p.skipped > 0 ? (
                  <div>
                    <dt>Не отправлено</dt>
                    <dd className="num">{count(p.skipped)}</dd>
                  </div>
                ) : null}
                <div>
                  <dt>Всего</dt>
                  <dd className="num">
                    {count(done(p))} из {count(p.total)}
                  </dd>
                </div>
              </dl>
              {b.channel === 'both' ? (
                <div className="bc-lanes">
                  {b.channels.map((lane) => (
                    <p key={lane.channel} className="bc-lane">
                      <strong>{lane.channel === 'bot' ? 'В бот' : 'На почту'}</strong>
                      <span className="muted">
                        {outcome(lane.progress, lane.channel)}
                        {b.status === 'sending' && lane.progress.pending > 0 ? ` · осталось ${count(lane.progress.pending)}` : ''}
                      </span>
                    </p>
                  ))}
                </div>
              ) : null}
            </div>
          ) : null}
          {b.notice ? <p className="warn-line bc-note">{b.notice}</p> : null}
          {b.mail_notice ? <p className="warn-line bc-note">{b.mail_notice}</p> : null}
          {b.queued_behind ? (
            <p className="cap bc-note">
              Ждёт очереди: сначала закончится <Link to={`${ROOT}/${b.queued_behind}`}>рассылка №{b.queued_behind}</Link>
            </p>
          ) : null}
          {b.status === 'scheduled' ? (
            <p className="bc-note">
              Получатели соберутся в момент отправки
              {reach ? (reachText(reach, b.channel) ? `, сейчас она ушла бы ${reachText(reach, b.channel)}` : ', сейчас отправлять некому') : ''}.
            </p>
          ) : null}
          {error ? (
            <p className="err bc-note" role="alert">
              {error}
            </p>
          ) : null}
          {confirm ? (
            <div className="bc-ask" role="group" aria-label="Подтверждение">
              {b.status === 'sending' ? (
                <p>
                  Остановить рассылку? <span className="muted">Тем, кому сообщение ещё не ушло, оно не придёт. У остальных останется.</span>
                </p>
              ) : (
                <p>
                  Отменить отправку? <span className="muted">Рассылка станет черновиком: её можно поправить и отправить снова.</span>
                </p>
              )}
              <div className="bc-ask-btns">
                <button type="button" className="btn-bad" onClick={() => void act('cancel')} disabled={busy}>
                  {busy ? 'Минуту…' : b.status === 'sending' ? 'Остановить' : 'Отменить отправку'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setConfirm(false)} disabled={busy}>
                  {b.status === 'sending' ? 'Продолжить отправку' : 'Оставить как есть'}
                </button>
              </div>
            </div>
          ) : (
            <div className="bc-actions">
              {b.status === 'sending' ? (
                <button type="button" className="btn-line hov" onClick={() => setConfirm(true)}>
                  Остановить рассылку
                </button>
              ) : null}
              {b.status === 'scheduled' ? (
                <button type="button" className="btn-line hov" onClick={() => setConfirm(true)}>
                  Отменить отправку
                </button>
              ) : null}
              {b.status === 'done' || b.status === 'canceled' ? (
                <button type="button" className="btn-line hov" onClick={() => void act('copy')} disabled={busy}>
                  {busy ? 'Копируем…' : 'Повторить как новую'}
                </button>
              ) : null}
            </div>
          )}
        </section>

        {b.problems.length ? (
          <section className="sec" aria-labelledby="bc-why-h">
            <div className="sec-head">
              <h2 id="bc-why-h" className="section-title">
                Почему не доставлено
              </h2>
              <span className="muted">
                {hasBot(b.channel)
                  ? 'Частые причины. Тем, у кого бот остановлен, сообщения в бот больше не уходят, пока они снова не откроют бота.'
                  : 'Частые причины. Адреса — в списке ниже.'}
              </span>
            </div>
            {b.problems.map((problem) => (
              <div key={`${problem.channel}:${problem.error}`} className="row bc-problem">
                <span>
                  {b.channel === 'both' ? (problem.channel === 'bot' ? 'Бот: ' : 'Почта: ') : ''}
                  {problem.error}
                </span>
                <span className="num">{count(problem.count)}</span>
              </div>
            ))}
          </section>
        ) : null}

        {b.failed_emails.length ? (
          <section className="sec" aria-labelledby="bc-failed-h">
            <div className="sec-head">
              <h2 id="bc-failed-h" className="section-title">
                Адреса с ошибкой
              </h2>
              <span className="muted">Что ответил почтовый сервер. В следующих рассылках письма на эти адреса уйдут снова.</span>
            </div>
            {b.failed_emails.map((row) => (
              <div key={row.email} className="row bc-failed">
                <span className="bc-failed-to">{row.email}</span>
                <span className="muted bc-failed-why">{row.error}</span>
              </div>
            ))}
            {mailFailed > b.failed_emails.length ? (
              <p className="cap bc-pad">И ещё {count(mailFailed - b.failed_emails.length)}</p>
            ) : null}
          </section>
        ) : null}

        <section className="sec" aria-labelledby="bc-about-h">
          <div className="sec-head">
            <h2 id="bc-about-h" className="section-title">
              Подробности
            </h2>
          </div>
          <dl className="bc-facts">
            <div>
              <dt>Куда</dt>
              <dd>{CHANNELS.find((c) => c.key === b.channel)?.title}</dd>
            </div>
            {b.subject && hasEmail(b.channel) ? (
              <div>
                <dt>Тема письма</dt>
                <dd>{b.subject}</dd>
              </div>
            ) : null}
            <div>
              <dt>Кому</dt>
              <dd>{b.audience_label}</dd>
            </div>
            <div>
              <dt>Создана</dt>
              <dd>{at(b.created_at)}</dd>
            </div>
            {b.started_at ? (
              <div>
                <dt>Начата</dt>
                <dd>{at(b.started_at)}</dd>
              </div>
            ) : null}
            {b.finished_at ? (
              <div>
                <dt>{b.status === 'canceled' ? 'Остановлена' : 'Закончена'}</dt>
                <dd>{at(b.finished_at)}</dd>
              </div>
            ) : null}
          </dl>
        </section>
        <div className="bc-inline">
          <ChannelPreview
            channel={b.channel}
            subject={b.subject ?? ''}
            text={b.text}
            image={b.image}
            buttons={b.buttons}
            brand={me.brand_name}
          />
        </div>
      </div>
      <aside className="bc-side">
        <ChannelPreview
          channel={b.channel}
          subject={b.subject ?? ''}
          text={b.text}
          image={b.image}
          buttons={b.buttons}
          brand={me.brand_name}
        />
      </aside>
    </div>
  );
}
