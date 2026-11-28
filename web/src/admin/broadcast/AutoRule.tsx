import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { plural, timeHM } from '../../lib/format';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import { BotWarning } from './List';
import {
  MessageFields,
  TestFields,
  checkMessage,
  messageFrom,
  subjectOf,
  toDraft,
  toSpec,
  type ButtonDraft,
  type MessageValue,
  type Note,
} from './Message';
import { ChannelPreview } from './Preview';
import { FilterBar } from '../clients/Conditions';
import { EMPTY, conditionsJson, fromAudience, type Conditions, type Kind } from '../clients/filters';
import {
  AUTO_ROOT,
  BASE,
  TRIGGER_HINTS,
  TRIGGER_WHEN,
  addresses,
  at,
  clients,
  count,
  failText,
  hasBot,
  hasEmail,
  timing,
  unitWord,
  type AutoReach,
  type Channel,
  type Localized,
  type Options,
  type Rule,
  type Trigger,
  type Unit,
} from './model';
import '../form.css';
import './broadcast.css';

type Failure = { text: string; disabled: boolean };

function failure(err: unknown, fallback: string): Failure {
  return {
    text: failText(err, fallback),
    disabled: err instanceof ApiError && err.code === 'module.disabled',
  };
}

export function RuleDetail() {
  const { id = 'new' } = useParams();
  const navigate = useNavigate();
  const phone = usePhone();
  const [options, setOptions] = useState<Options | null>(null);
  const [current, setCurrent] = useState<Rule | null>(null);
  const [version, setVersion] = useState(0);
  const [loading, setLoading] = useState(id !== 'new');
  const [error, setError] = useState<Failure | null>(null);
  const latest = useRef<Rule | null>(null);
  latest.current = current;

  useEffect(() => {
    api<Options>(`${BASE}/options`)
      .then(setOptions)
      .catch((err) => setError(failure(err, 'Раздел не загрузился')));
  }, []);

  useEffect(() => {
    if (id === 'new') {
      setCurrent(null);
      setVersion((v) => v + 1);
      setLoading(false);
      return;
    }
    if (latest.current && String(latest.current.id) === id) {
      setLoading(false);
      return;
    }
    let alive = true;
    setLoading(true);
    api<Rule>(`${BASE}/auto/${encodeURIComponent(id)}`)
      .then((rule) => {
        if (alive) {
          setCurrent(rule);
          setVersion((v) => v + 1);
          setError(null);
        }
      })
      .catch((err) => {
        if (alive) {
          setError(failure(err, 'Правило не загрузилось'));
        }
      })
      .finally(() => {
        if (alive) {
          setLoading(false);
        }
      });
    return () => {
      alive = false;
    };
  }, [id]);

  const saved = useCallback(
    (rule: Rule) => {
      setCurrent(rule);
      if (String(rule.id) !== id) {
        navigate(`${AUTO_ROOT}/${rule.id}`, { replace: true });
      }
    },
    [id, navigate],
  );

  const title = current ? current.title : id === 'new' ? 'Новое правило' : 'Правило';

  let body;
  if (error) {
    body = (
      <section className="card card-pad" role="alert">
        <p className="err" style={{ fontSize: 15 }}>
          {error.text}
        </p>
        {error.disabled ? (
          <p className="muted">
            Включить модуль можно в разделе <Link to="/admin/modules/broadcast">«Модули»</Link>.
          </p>
        ) : null}
      </section>
    );
  } else if (!options || loading) {
    body = <p className="muted">Загружаем…</p>;
  } else {
    body = <RuleEditor key={version} options={options} rule={current} onSaved={saved} onDeleted={() => navigate(AUTO_ROOT)} />;
  }

  if (phone) {
    return (
      <div className="hp bc-phone">
        <PhoneHeader title={title} back={{ to: AUTO_ROOT, label: 'Все правила' }} />
        <BotWarning options={options} />
        {body}
      </div>
    );
  }

  return (
    <>
      <Link to={AUTO_ROOT} className="bc-back hov">
        <Icon name="chevron-left" size={18} />
        Все правила
      </Link>
      <h1 className="page-title tight">{title}</h1>
      <BotWarning options={options} />
      {body}
    </>
  );
}

function reachLine(reach: AutoReach, channel: Channel): string | null {
  const parts = [
    hasBot(channel) && reach.recipients > 0 ? `${clients(reach.recipients)} в${' '}бот` : null,
    hasEmail(channel) && reach.emails > 0 ? `письмом на${' '}${addresses(reach.emails)}` : null,
  ].filter((part): part is string => part !== null);
  return parts.length ? parts.join(' и ') : null;
}

function Work({ rule }: { rule: Rule }) {
  const stats = rule.stats;
  const p = stats.progress;
  return (
    <section className="sec" aria-labelledby="ar-work-h">
      <div className="sec-head bc-head">
        <h2 id="ar-work-h" className="section-title">
          Работа правила
        </h2>
        <span className={`bc-status ${rule.enabled ? '' : 'muted'}`}>
          {rule.enabled ? (rule.checked_at ? `Включено, проверено в ${timeHM(rule.checked_at)}` : 'Включено, проверка в ближайшие минуты') : 'Выключено'}
        </span>
      </div>
      {stats.cases === 0 ? (
        <p className="cap bc-note">Ещё не срабатывало{rule.enabled ? ': сообщение уйдёт, когда у клиента наступит событие' : ''}</p>
      ) : (
        <>
          <div className="bc-progress">
            <dl className="bc-dl">
              <div>
                <dt>Сработало</dt>
                <dd className="num">{count(stats.cases)}</dd>
              </div>
              <div>
                <dt>{rule.channel === 'bot' ? 'Доставлено' : 'Отправлено'}</dt>
                <dd className="num">{count(p.sent)}</dd>
              </div>
              <div>
                <dt>Не доставлено</dt>
                <dd className="num">{count(p.failed)}</dd>
              </div>
              {hasBot(rule.channel) ? (
                <div>
                  <dt>Бот остановлен</dt>
                  <dd className="num">{count(p.blocked)}</dd>
                </div>
              ) : null}
              {p.pending > 0 ? (
                <div>
                  <dt>В очереди</dt>
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
                <dt>Без связи</dt>
                <dd className="num">{count(stats.unreachable)}</dd>
              </div>
            </dl>
          </div>
          <p className="cap bc-note">
            {stats.last_sent_at ? `Последнее сообщение — ${at(stats.last_sent_at)}. ` : ''}
            Без связи — нет Telegram или бот остановлен, нет почты или адрес отписался. Такой случай засчитывается и позже не повторяется.
          </p>
        </>
      )}
      {rule.notice ? <p className="warn-line bc-note">{rule.notice}</p> : null}
      {rule.sending_notices.map((line) => (
        <p key={line} className="warn-line bc-note">
          {line}
        </p>
      ))}
      {rule.problems.map((problem) => (
        <div key={`${problem.channel}:${problem.error}`} className="row bc-problem">
          <span>
            {rule.channel === 'both' ? (problem.channel === 'bot' ? 'Бот: ' : 'Почта: ') : ''}
            {problem.error}
          </span>
          <span className="num">{count(problem.count)}</span>
        </div>
      ))}
    </section>
  );
}

type Confirm = 'enable' | 'delete' | null;

type EditorProps = {
  options: Options;
  rule: Rule | null;
  onSaved: (rule: Rule) => void;
  onDeleted: () => void;
};

type English = { subject: string; text: string; buttons: ButtonDraft[] };

function englishOf(en: Localized | null): English | null {
  return en ? { subject: en.subject ?? '', text: en.text, buttons: en.buttons.map(toDraft) } : null;
}

function englishBody(en: English | null) {
  return en ? { subject: en.subject.trim().replace(/\s+/g, ' ') || null, text: en.text, buttons: en.buttons.map(toSpec) } : null;
}

const RULE_EXCLUDE: Kind[] = ['status', 'term', 'registered'];

function ruleExclude(option: { tariff: boolean; autorenew: boolean } | undefined): Kind[] {
  const out = [...RULE_EXCLUDE];
  if (option && !option.tariff) {
    out.push('tariff', 'category');
  }
  if (option && !option.autorenew) {
    out.push('autorenew');
  }
  return out;
}

function RuleEditor({ options, rule, onSaved, onDeleted }: EditorProps) {
  const { me, can } = useSession();
  const phone = usePhone();
  const auto = options.auto;
  const start = rule ? null : TRIGGER_WHEN.no_subscription;
  const [id, setId] = useState<number | null>(rule?.id ?? null);
  const [message, setMessage] = useState<MessageValue>(() => messageFrom(rule));
  const [english, setEnglish] = useState<English | null>(() => englishOf(rule?.en ?? null));
  const [lang, setLang] = useState<'ru' | 'en'>('ru');
  const [trigger, setTrigger] = useState<Trigger>(rule?.trigger ?? 'no_subscription');
  const [amount, setAmount] = useState(String(rule ? rule.amount : (start?.amount ?? 1)));
  const [unit, setUnit] = useState<Unit>(rule?.unit ?? start?.unit ?? 'days');
  const [percent, setPercent] = useState(String(rule?.percent ?? auto.percent_default));
  const [withRenewal, setWithRenewal] = useState(rule?.with_autorenew ?? false);
  const [enabled, setEnabled] = useState(rule?.enabled ?? false);
  const [reach, setReach] = useState<AutoReach | null>(null);
  const [reachError, setReachError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<Note | null>(null);
  const [tried, setTried] = useState(false);
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [who, setWho] = useState<Conditions>(() => (rule?.audience ? fromAudience({ kind: 'filter', ...rule.audience }) : { ...EMPTY }));
  const exclude = ruleExclude(auto.triggers.find((t) => t.key === trigger));

  const { channel, text, image, buttons } = message;
  const englishShown: MessageValue | null = english ? { ...message, subject: english.subject, text: english.text, buttons: english.buttons } : null;
  const shown = lang === 'en' && englishShown ? englishShown : message;
  const change = (patch: Partial<MessageValue>) => {
    if (lang === 'en' && english) {
      const shared: Partial<MessageValue> = {};
      if ('channel' in patch) {
        shared.channel = patch.channel;
      }
      if ('image' in patch) {
        shared.image = patch.image;
      }
      setMessage((current) => ({ ...current, ...shared }));
      setEnglish((current) =>
        current
          ? {
              subject: patch.subject ?? current.subject,
              text: patch.text ?? current.text,
              buttons: patch.buttons ?? current.buttons,
            }
          : current,
      );
      return;
    }
    setMessage((current) => ({ ...current, ...patch }));
  };
  const checkRu = checkMessage(message, options, tried);
  const checkEn = englishShown
    ? checkMessage({ ...englishShown, subject: englishShown.subject.trim() ? englishShown.subject : message.subject }, options, tried)
    : null;
  const check = lang === 'en' && checkEn ? checkEn : checkRu;
  const option = auto.triggers.find((t) => t.key === trigger);
  const byPercent = option?.percent ?? false;
  const fixed = option?.fixed ?? false;
  const minAmount = option?.min ?? 1;
  const maxAmount = unit === 'hours' ? auto.hours_max : auto.days_max;
  const amountCount = Number.parseInt(amount, 10);
  const amountOk =
    byPercent ||
    fixed ||
    (/^\d+$/.test(amount.trim()) && amountCount >= minAmount && amountCount <= maxAmount && (trigger !== 'idle' || unit === 'days' || amountCount >= 24));
  const percentCount = Number.parseInt(percent, 10);
  const percentOk = !byPercent || (/^\d+$/.test(percent.trim()) && percentCount >= auto.percent_min && percentCount <= auto.percent_max);
  const whenOk = amountOk && percentOk;
  const renewal = trigger === 'expiring' && withRenewal;
  const subject = subjectOf(message);
  const whenProblem = !amountOk
    ? trigger === 'idle' && unit === 'hours' && amountCount < 24 && amountCount >= minAmount
      ? 'Без подключений — не меньше суток'
      : `Число ${unit === 'hours' ? 'часов' : 'дней'} от ${minAmount} до ${maxAmount}`
    : !percentOk
      ? `Порог от ${auto.percent_min} до ${auto.percent_max}%`
      : null;

  const body = useMemo(
    () => ({
      trigger,
      amount: byPercent || fixed || !amountOk ? 0 : amountCount,
      unit: byPercent || fixed ? 'days' : unit,
      percent: byPercent && percentOk ? percentCount : null,
      with_autorenew: renewal,
      channel,
      subject,
      text,
      image_id: image?.id ?? null,
      buttons: buttons.map(toSpec),
      en: englishBody(english),
      audience: conditionsJson(who),
    }),
    [trigger, byPercent, fixed, amountOk, amountCount, unit, percentOk, percentCount, renewal, channel, subject, text, image, buttons, english, who],
  );
  const bodyKey = JSON.stringify(body);
  const [savedKey, setSavedKey] = useState(() => (rule ? bodyKey : ''));
  const dirty = bodyKey !== savedKey;

  const probeKey = whenOk
    ? JSON.stringify({ trigger, amount: body.amount, unit: body.unit, percent: body.percent, with_autorenew: renewal, id, audience: body.audience })
    : '';
  useEffect(() => {
    if (!probeKey) {
      setReach(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      api<AutoReach>(`${BASE}/auto/reach`, { method: 'POST', body: JSON.parse(probeKey) as object })
        .then((value) => {
          if (alive) {
            setReach(value);
            setReachError(null);
          }
        })
        .catch((err) => {
          if (alive) {
            setReach(null);
            setReachError(failText(err, 'Не получилось посчитать'));
          }
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [probeKey]);

  const pickTrigger = (next: Trigger) => {
    setTrigger(next);
    setNote(null);
    const nextOption = auto.triggers.find((t) => t.key === next);
    setWho((current) => ({
      ...current,
      tariff: nextOption?.tariff === false ? [] : current.tariff,
      category: nextOption?.tariff === false ? [] : current.category,
      autorenew: nextOption?.autorenew === false ? null : current.autorenew,
    }));
    if (!rule) {
      setAmount(String(TRIGGER_WHEN[next].amount));
      setUnit(TRIGGER_WHEN[next].unit);
    }
  };

  const contentOk = checkRu.contentOk && (checkEn?.messageOk ?? true);
  const canSave = contentOk && whenOk && busy === null;
  const canEnable = canSave && !checkRu.mailOff;
  const waiting = reach ? (hasBot(channel) ? reach.recipients : 0) + (hasEmail(channel) ? reach.emails : 0) : 0;
  const problemText = !whenOk
    ? (whenProblem ?? 'Проверьте, когда отправить')
    : !checkRu.contentOk
      ? 'Проверьте сообщение: в нём есть ошибки'
      : 'Проверьте текст на английском: в нём есть ошибки';

  const apply = (saved: Rule) => {
    setId(saved.id);
    setEnabled(saved.enabled);
    setMessage((current) => ({ ...current, subject: saved.subject ?? '', text: saved.text, buttons: saved.buttons.map(toDraft) }));
    const savedEnglish = englishOf(saved.en);
    setEnglish(savedEnglish);
    setSavedKey(
      JSON.stringify({
        trigger: saved.trigger,
        amount: saved.amount,
        unit: saved.unit,
        percent: saved.percent,
        with_autorenew: saved.with_autorenew,
        channel: saved.channel,
        subject: subjectOf({ ...message, subject: saved.subject ?? '', channel: saved.channel }),
        text: saved.text,
        image_id: saved.image?.id ?? null,
        buttons: saved.buttons.map((b) => toSpec(toDraft(b))),
        en: englishBody(savedEnglish),
        audience: saved.audience && Object.keys(saved.audience).length ? saved.audience : null,
      }),
    );
    onSaved(saved);
  };

  const save = async (on: boolean) => {
    setTried(true);
    if (!canSave) {
      setNote({ text: problemText, tone: 'bad' });
      return;
    }
    setBusy('save');
    setNote(null);
    try {
      const saved =
        id === null
          ? await api<Rule>(`${BASE}/auto`, { method: 'POST', body: { ...body, enabled: on } })
          : await api<Rule>(`${BASE}/auto/${id}`, { method: 'PUT', body: { ...body, enabled: on } });
      apply(saved);
      setConfirm(null);
      setNote({ text: on ? (enabled ? 'Изменения сохранены' : 'Правило включено') : enabled ? 'Правило выключено' : 'Сохранено', tone: 'ok' });
    } catch (err) {
      setConfirm(null);
      setNote({ text: failText(err, 'Правило не сохранилось'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const flip = async (on: boolean) => {
    if (dirty || id === null) {
      await save(on);
      return;
    }
    setBusy('switch');
    setNote(null);
    try {
      const saved = await api<Rule>(`${BASE}/auto/${id}/switch`, { method: 'POST', body: { enabled: on } });
      apply(saved);
      setConfirm(null);
      setNote({ text: on ? 'Правило включено' : 'Правило выключено', tone: 'ok' });
    } catch (err) {
      setConfirm(null);
      setNote({ text: failText(err, on ? 'Не получилось включить' : 'Не получилось выключить'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const askEnable = () => {
    setTried(true);
    if (!canEnable) {
      setNote({
        text: checkRu.mailOff ? 'Почта не отправляет: выберите «В бот» или настройте почту' : problemText,
        tone: 'bad',
      });
      return;
    }
    if (waiting > 0) {
      setConfirm('enable');
    } else {
      void flip(true);
    }
  };

  const remove = async () => {
    if (id === null) {
      onDeleted();
      return;
    }
    setBusy('delete');
    try {
      await api(`${BASE}/auto/${id}`, { method: 'DELETE' });
      onDeleted();
    } catch (err) {
      setConfirm(null);
      setNote({ text: failText(err, 'Правило не удалилось'), tone: 'bad' });
      setBusy(null);
    }
  };

  const preview = (
    <ChannelPreview
      key={lang}
      channel={channel}
      subject={shown.subject.trim() ? shown.subject : message.subject}
      text={shown.text}
      image={image}
      buttons={shown.buttons.map(toSpec)}
      brand={me.brand_name}
    />
  );
  const spec = TRIGGER_WHEN[trigger];
  const recipients = reach ? reachLine(reach, channel) : null;
  const languages = (
    <div className="row ar-langs">
      <span className="rl">Язык</span>
      <div className="rc">
        <span className="segs" role="group" aria-label="Язык текста">
          {(['ru', 'en'] as const).map((value) => (
            <button key={value} type="button" aria-pressed={lang === value} onClick={() => setLang(value)}>
              {value === 'ru' ? 'Русский' : 'English'}
            </button>
          ))}
        </span>
        <span className="cap">
          {lang === 'ru'
            ? english
              ? 'Клиенты, выбравшие английский, получат английский текст. Канал и картинка общие.'
              : 'Клиенты, выбравшие английский, получат этот текст: английского пока нет.'
            : auto.english
              ? 'Этот текст получат клиенты, выбравшие английский. Канал и картинка общие.'
              : 'Английский выключен в настройках «Языки»: текст сохранится и заработает, когда его включат.'}
        </span>
        {lang === 'en' && english ? (
          <button
            type="button"
            className="bc-link bad"
            onClick={() => {
              setEnglish(null);
              setLang('ru');
            }}
          >
            Убрать английский текст
          </button>
        ) : null}
      </div>
    </div>
  );

  return (
    <div className="bc-grid">
      <div className="bc-col">
        {rule ? <Work rule={rule} /> : null}
        <section className="sec" aria-labelledby="ar-when-h">
          <div className="sec-head">
            <h2 id="ar-when-h" className="section-title">
              Когда отправить
            </h2>
            <span className="muted">
              Правило проверяет клиентов раз в {auto.every_minutes}&nbsp;минут и пишет каждому один раз на случай. Тариф и другие условия — в разделе «Кому».
            </span>
          </div>
          <div className="row">
            <div className="rc bc-opts" role="radiogroup" aria-label="Событие">
              {auto.triggers.map((t) => (
                <div key={t.key}>
                  <label className="bc-opt">
                    <input type="radio" name="ar-trigger" checked={trigger === t.key} onChange={() => pickTrigger(t.key)} />
                    <span className="bc-opt-text">
                      <span className="bc-opt-title">{t.title}</span>
                      <span className="cap">{TRIGGER_HINTS[t.key]}</span>
                    </span>
                  </label>
                </div>
              ))}
            </div>
          </div>
          {fixed ? (
            <div className="row">
              <span className="rl">Срок</span>
              <div className="rc">
                <span className="ar-fixed">Сразу, как только у подписки заняты все устройства</span>
                <span className="cap">Ещё раз — только если лимит устройств у подписки изменится</span>
              </div>
            </div>
          ) : (
          <div className="row">
            <span className="rl">{byPercent ? 'Порог' : 'Срок'}</span>
            <div className="rc">
              {byPercent ? (
                <span className="bc-extra ar-days">
                  <span>{spec.before}</span>
                  <input
                    className="fld"
                    value={percent}
                    inputMode="numeric"
                    aria-label="Доля израсходованного трафика, процентов"
                    aria-invalid={!percentOk}
                    onChange={(e) => {
                      setPercent(e.target.value.replace(/[^\d]/g, '').slice(0, 3));
                      setNote(null);
                    }}
                  />
                  <span>% {spec.after}</span>
                </span>
              ) : (
                <span className="bc-extra ar-days">
                  <span>{spec.before}</span>
                  <input
                    className="fld"
                    value={amount}
                    inputMode="numeric"
                    aria-label={unit === 'hours' ? 'Число часов' : 'Число дней'}
                    aria-invalid={!amountOk}
                    onChange={(e) => {
                      setAmount(e.target.value.replace(/[^\d]/g, '').slice(0, 3));
                      setNote(null);
                    }}
                  />
                  <select
                    className="field ar-unit"
                    value={unit}
                    aria-label="Часы или дни"
                    onChange={(e) => {
                      setUnit(e.target.value as Unit);
                      setNote(null);
                    }}
                  >
                    {(['hours', 'days'] as Unit[]).map((value) => (
                      <option key={value} value={value}>
                        {unitWord(Number.isNaN(amountCount) ? 5 : amountCount, value)}
                      </option>
                    ))}
                  </select>
                  <span>{spec.after}</span>
                </span>
              )}
              {whenProblem ? (
                <span className="err">{whenProblem}</span>
              ) : (
                <span className="cap">
                  Сообщение уйдёт{byPercent ? ',' : ''} {timing(trigger, amountCount || 0, unit, byPercent ? percentCount : null)}
                  {minAmount === 0 && !byPercent ? '. Ноль — сразу после окончания' : ''}
                  {byPercent ? '. Только у подписок с лимитом трафика' : ''}
                </span>
              )}
            </div>
          </div>
          )}
          {trigger === 'expiring' ? (
            <div className="row">
              <div className="rc">
                <label className="bc-opt">
                  <input
                    type="checkbox"
                    className="ar-check"
                    checked={withRenewal}
                    onChange={(e) => {
                      setWithRenewal(e.target.checked);
                      setNote(null);
                    }}
                  />
                  <span className="bc-opt-text">
                    <span className="bc-opt-title">Писать и тем, у кого включено автопродление</span>
                    <span className="cap">Без этой отметки правило таким подпискам не пишет: бот сам спишет оплату или напомнит пополнить баланс</span>
                  </span>
                </label>
              </div>
            </div>
          ) : null}
        </section>

        <section className="sec" aria-labelledby="ar-who-h">
          <div className="sec-head">
            <h2 id="ar-who-h" className="section-title">
              Кому
            </h2>
            <span className="muted">Необязательно. Без условий правило пишет каждому, кто попал под событие.</span>
          </div>
          <div className="row">
            <div className="rc">
              <FilterBar value={who} onChange={setWho} options={options.filter} currency={me.currency} money={can('money')} mode="rule" phone={phone} exclude={exclude} />
              {option && !option.tariff ? <span className="cap">У этого события ещё нет подписки, поэтому тариф и автопродление здесь не выбираются</span> : null}
              {option && option.tariff && !option.autorenew ? <span className="cap">Тариф — тот, который клиент выбрал в неоплаченном счёте</span> : null}
            </div>
          </div>
          <div className="bc-reach" aria-live="polite">
            {reachError ? (
              <span className="err">{reachError}</span>
            ) : !whenOk ? (
              <span className="muted">{byPercent ? 'Уточните порог' : 'Уточните срок'}</span>
            ) : reach === null ? (
              <span className="muted">Считаем…</span>
            ) : reach.cases === 0 ? (
              <span className="muted">
                {enabled ? 'Новых случаев сейчас нет: правило подхватывает их по мере наступления' : 'Сейчас под условие никто не попадает'}
              </span>
            ) : (
              <>
                <span className="bc-reach-line">
                  <span>
                    Под условие сейчас попадают: <strong className="num">{count(reach.cases)}</strong>
                  </span>
                  <span className="muted">
                    {enabled ? `им уйдёт при ближайшей проверке` : `после включения им уйдёт ${recipients ?? 'никому: с ними нет связи'}`}
                  </span>
                </span>
                {hasBot(channel) && reach.stopped > 0 ? <span className="muted">У {count(reach.stopped)} бот остановлен — в бот им не отправим</span> : null}
                {hasEmail(channel) && reach.unsubscribed > 0 ? (
                  <span className="muted">
                    {count(reach.unsubscribed)} {plural(reach.unsubscribed, 'адрес отписался', 'адреса отписались', 'адресов отписались')} от рассылок — письмо им не уйдёт
                  </span>
                ) : null}
              </>
            )}
          </div>
        </section>

        {lang === 'en' && !english ? (
          <section className="sec" aria-labelledby="ar-en-h">
            <div className="sec-head">
              <h2 id="ar-en-h" className="section-title">
                Сообщение
              </h2>
              <span className="muted">Текст для клиентов, которые выбрали английский язык.</span>
            </div>
            {languages}
            <div className="row">
              <div className="rc">
                <span>Английского текста пока нет: клиенты с английским получат русский.</span>
                <span>
                  <button
                    type="button"
                    className="btn-line hov"
                    onClick={() => setEnglish({ subject: '', text: '', buttons: message.buttons.map((b) => ({ ...b, text: '' })) })}
                  >
                    Добавить английский текст
                  </button>
                </span>
              </div>
            </div>
          </section>
        ) : (
          <MessageFields
            options={options}
            value={shown}
            onChange={change}
            check={check}
            tried={tried}
            busy={busy}
            setBusy={setBusy}
            onNote={setNote}
            preview={preview}
            placeholder={
              lang === 'en'
                ? 'For example: Your subscription ends in 3 days. Renew it now to keep the VPN running.'
                : 'Например: Подписка закончится через 3 дня. Продлите сейчас, чтобы VPN не отключился.'
            }
            subjectPlaceholder={lang === 'en' ? 'Пусто — тема как у русского письма' : 'Например: Подписка скоро закончится'}
            extra={languages}
          />
        )}

        <TestFields
          options={options}
          value={shown}
          check={check}
          busy={busy}
          setBusy={setBusy}
          onTried={() => setTried(true)}
          lead="Отправьте сообщение себе и посмотрите, как его увидит клиент."
        />

        <section className="sec" aria-labelledby="ar-save-h">
          <div className="sec-head">
            <h2 id="ar-save-h" className="section-title">
              {id === null ? 'Сохранение' : 'Правило'}
            </h2>
            <span className="muted">
              {id === null
                ? 'Можно сохранить выключенным и включить позже — в списке правил или здесь.'
                : enabled
                  ? 'Включено: сообщения уходят сами. Выключение останавливает и те, что ещё ждут в очереди.'
                  : 'Выключено: сообщения не уходят. Случаи, пропущенные больше суток назад, после включения не догоняются.'}
            </span>
          </div>
          {confirm === 'enable' ? (
            <div className="bc-ask" role="group" aria-label="Подтверждение включения">
              <p>
                Правило включится, и в ближайшие {auto.every_minutes}&nbsp;минут сообщение уйдёт <strong>{recipients ?? 'получателям'}</strong> — у них событие
                наступило за последние сутки. <span className="muted">Дальше — каждому, у кого оно наступит.</span>
              </p>
              <div className="bc-ask-btns">
                <button type="button" className="btn-main btn" onClick={() => void flip(true)} disabled={busy !== null}>
                  {busy ? 'Включаем…' : 'Включить'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setConfirm(null)} disabled={busy !== null}>
                  Не включать
                </button>
              </div>
            </div>
          ) : confirm === 'delete' ? (
            <div className="bc-ask" role="group" aria-label="Подтверждение удаления">
              <p>
                Удалить правило? <span className="muted">Сообщения, которые ещё ждут в очереди, не уйдут. Вернуть правило не получится.</span>
              </p>
              <div className="bc-ask-btns">
                <button type="button" className="btn-bad" onClick={() => void remove()} disabled={busy !== null}>
                  {busy === 'delete' ? 'Удаляем…' : 'Удалить'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setConfirm(null)} disabled={busy !== null}>
                  Оставить
                </button>
              </div>
            </div>
          ) : (
            <div className="bc-actions">
              {id === null ? (
                <>
                  <button type="button" className="btn-main btn" onClick={askEnable} disabled={busy !== null || (tried && !canEnable)}>
                    Сохранить и включить
                  </button>
                  <button type="button" className="btn-line hov" onClick={() => void save(false)} disabled={busy !== null}>
                    {busy === 'save' ? 'Сохраняем…' : 'Сохранить выключенным'}
                  </button>
                </>
              ) : enabled ? (
                <>
                  <button type="button" className="btn-main btn" onClick={() => void save(true)} disabled={busy !== null || !dirty}>
                    {busy === 'save' ? 'Сохраняем…' : 'Сохранить изменения'}
                  </button>
                  <button type="button" className="btn-line hov" onClick={() => void flip(false)} disabled={busy !== null}>
                    {dirty ? 'Сохранить и выключить' : 'Выключить'}
                  </button>
                </>
              ) : (
                <>
                  <button type="button" className="btn-main btn" onClick={askEnable} disabled={busy !== null || (tried && !canEnable)}>
                    {dirty ? 'Сохранить и включить' : 'Включить'}
                  </button>
                  <button type="button" className="btn-line hov" onClick={() => void save(false)} disabled={busy !== null || !dirty}>
                    {busy === 'save' ? 'Сохраняем…' : 'Сохранить изменения'}
                  </button>
                </>
              )}
              {id !== null ? (
                <button type="button" className="bc-link bad" onClick={() => setConfirm('delete')} disabled={busy !== null}>
                  Удалить правило
                </button>
              ) : null}
              <span role="status" className={note?.tone === 'bad' ? 'bad' : undefined}>
                {note?.text ?? (id !== null && dirty ? 'Есть несохранённые изменения' : '')}
              </span>
            </div>
          )}
        </section>
      </div>
      <aside className="bc-side">{preview}</aside>
    </div>
  );
}
