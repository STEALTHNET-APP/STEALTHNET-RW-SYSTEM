import { Fragment, type DragEvent, type KeyboardEvent, type ReactNode } from 'react';
import { Icon } from '../../lib/icons';
import { TelegramText } from '../pages/telegram';
import type { Lang, Slot } from './model';

export type KeyMarks = {
  selected?: string | null;
  ghost?: string | null;
  partial?: (slot: Slot) => boolean;
  faded?: (slot: Slot) => boolean;
  drop?: { key: string; after: boolean } | null;
};

export type KeyEvents = {
  onPick?: (key: string) => void;
  onKeyDown?: (key: string, event: KeyboardEvent<HTMLButtonElement>) => void;
  onDragStart?: (key: string, event: DragEvent<HTMLButtonElement>) => void;
  onDragEnd?: () => void;
  onDragOver?: (key: string, event: DragEvent<HTMLButtonElement>) => void;
  onDrop?: (key: string, event: DragEvent<HTMLButtonElement>) => void;
};

type Props = {
  html: string;
  rows: Slot[][];
  lang: Lang;
  brand: string;
  compact?: boolean;
  marks?: KeyMarks;
  events?: KeyEvents;
  renderRow?: (row: Slot[], index: number, keys: ReactNode) => ReactNode;
  empty?: string;
};

export function labelOf(slot: Slot, lang: Lang): string {
  return lang === 'en' ? slot.text_en : slot.text;
}

function TgKey({ slot, lang, marks, events }: { slot: Slot; lang: Lang; marks?: KeyMarks; events?: KeyEvents }) {
  const classes = [
    'tg-key',
    slot.style ?? '',
    marks?.selected === slot.key ? 'sel' : '',
    marks?.ghost === slot.key ? 'ghost' : '',
    marks?.partial?.(slot) ? 'part' : '',
    marks?.faded?.(slot) ? 'faded' : '',
  ]
    .filter(Boolean)
    .join(' ');
  const pickable = Boolean(events?.onPick);
  const draggable = Boolean(events?.onDragStart);
  const drop = marks?.drop?.key === slot.key ? (marks.drop.after ? 'after' : 'before') : undefined;
  return (
    <button
      type="button"
      className={classes}
      data-key={slot.key}
      data-drop={drop}
      draggable={draggable || undefined}
      aria-pressed={pickable ? marks?.selected === slot.key : undefined}
      tabIndex={pickable ? 0 : -1}
      onClick={pickable ? () => events?.onPick?.(slot.key) : undefined}
      onKeyDown={events?.onKeyDown ? (event) => events.onKeyDown?.(slot.key, event) : undefined}
      onDragStart={draggable ? (event) => events?.onDragStart?.(slot.key, event) : undefined}
      onDragEnd={events?.onDragEnd}
      onDragOver={events?.onDragOver ? (event) => events.onDragOver?.(slot.key, event) : undefined}
      onDrop={events?.onDrop ? (event) => events.onDrop?.(slot.key, event) : undefined}
    >
      {draggable ? <span className="tg-grip" aria-hidden="true" /> : null}
      {slot.emoji ? <span className="tg-emoji" title="Премиум-эмодзи" aria-hidden="true" /> : null}
      <span className="tg-key-text">{labelOf(slot, lang)}</span>
      {slot.kind === 'link' || slot.kind === 'web_app' ? <Icon name="external" size={11} stroke={2.4} className="tg-corner" /> : null}
    </button>
  );
}

export function TgPreview({ html, rows, lang, brand, compact, marks, events, renderRow, empty }: Props) {
  return (
    <div className={compact ? 'tg-win compact' : 'tg-win'} lang={lang}>
      <div className="tg-head">
        <span className="tg-ava" aria-hidden="true">
          {(brand.trim().charAt(0) || 'B').toUpperCase()}
        </span>
        <span className="tg-who">
          <b>{brand}</b>
          <span>{lang === 'en' ? 'bot' : 'бот'}</span>
        </span>
      </div>
      <div className="tg-chat">
        <div className="tg-msg">
          <div className="tg-bubble">
            <TelegramText html={html} />
            <span className="tg-time">12:41</span>
          </div>
          <div className="tg-kb">
            {rows.length === 0 && empty ? <span className="tg-empty">{empty}</span> : null}
            {rows.map((row, index) => {
              const keys = (
                <div className="tg-row">
                  {row.map((slot) => (
                    <TgKey key={slot.key} slot={slot} lang={lang} marks={marks} events={events} />
                  ))}
                </div>
              );
              return <Fragment key={row.map((s) => s.key).join(' ')}>{renderRow ? renderRow(row, index, keys) : keys}</Fragment>;
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
