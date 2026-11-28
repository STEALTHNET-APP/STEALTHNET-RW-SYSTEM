import { useRef, useState, type DragEvent, type ReactNode } from 'react';
import { uploadWithProgress } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { ACCEPT_ANY, ACCEPT_PHOTO, clock, fileProblem, kindOf, megabytes, wholeMb, type MediaKind } from './files';
import { BASE, count, failText, hasBot, hasEmail, imageUrl, isVideo, type Channel, type Image, type Options } from './model';
import type { Note } from './Message';

type Props = {
  options: Options;
  channel: Channel;
  image: Image | null;
  busy: string | null;
  setBusy: (busy: string | null) => void;
  onChange: (image: Image | null) => void;
  onNote: (note: Note | null) => void;
};

type Sending = { kind: MediaKind; loaded: number; total: number };

function Uploading({ sending, onCancel }: { sending: Sending; onCancel: () => void }) {
  const share = sending.total > 0 ? Math.min(100, Math.round((sending.loaded / sending.total) * 100)) : 0;
  return (
    <div className="bc-sending" role="status">
      <span className="bc-sending-line">
        <span>
          {sending.kind === 'video' ? 'Загружаем видео' : 'Загружаем картинку'}: {megabytes(sending.loaded)} из {megabytes(sending.total)}
        </span>
        <button type="button" className="bc-link" onClick={onCancel}>
          Отменить
        </button>
      </span>
      <span className="bc-bar wide" role="progressbar" aria-label="Загрузка файла" aria-valuemin={0} aria-valuemax={100} aria-valuenow={share}>
        <span style={{ width: `${share}%` }} />
      </span>
    </div>
  );
}

function Attached({ image, busy, onReplace, onRemove }: { image: Image; busy: string | null; onReplace: () => void; onRemove: () => void }) {
  const src = imageUrl(image.id);
  const video = isVideo(image);
  return (
    <div className="bc-image">
      {video ? (
        <span className="bc-thumb bc-frame">
          <video src={`${src}#t=0.1`} preload="metadata" muted playsInline aria-label="Видео сообщения" />
          <span className="bc-time">{clock(image.duration)}</span>
        </span>
      ) : (
        <img src={src} alt="Картинка сообщения" className="bc-thumb" />
      )}
      <span className="bc-image-info">
        <span>{video ? `Видео ${clock(image.duration)}` : `${image.width} × ${image.height}`}</span>
        <span className="cap">
          {video ? `${image.width} × ${image.height}, MP4` : image.content_type === 'image/png' ? 'PNG' : 'JPEG'}, {megabytes(image.size)}
        </span>
      </span>
      <button type="button" className="btn-line hov" onClick={onReplace} disabled={busy !== null}>
        Заменить
      </button>
      <button type="button" className="bc-link bad" onClick={onRemove} disabled={busy !== null}>
        Убрать
      </button>
    </div>
  );
}

function ChannelNote({ channel, video }: { channel: Channel; video: boolean }) {
  if (video && !hasBot(channel)) {
    return <span className="err">Видео уходит только в Telegram: для письма уберите видео или загрузите картинку.</span>;
  }
  if (video && hasEmail(channel)) {
    return <span className="warn-line">В письмо видео не попадёт: письмо уйдёт с текстом и кнопками.</span>;
  }
  if (!hasBot(channel)) {
    return <span className="cap">В письмо можно приложить только картинку: видео уходит только в Telegram.</span>;
  }
  return null;
}

export function MediaBlock({ options, channel, image, busy, setBusy, onChange, onNote }: Props) {
  const file = useRef<HTMLInputElement>(null);
  const control = useRef<AbortController | null>(null);
  const [sending, setSending] = useState<Sending | null>(null);
  const [over, setOver] = useState(false);
  const bot = hasBot(channel);
  const limits = options.limits;

  const start = async (picked: File | undefined) => {
    if (!picked || busy !== null) {
      return;
    }
    const problem = fileProblem(picked, limits, bot);
    const kind = kindOf(picked);
    if (problem !== null || (kind !== 'photo' && kind !== 'video')) {
      onNote({ text: problem ?? 'Нужна картинка JPEG или PNG или видео MP4', tone: 'bad' });
      return;
    }
    const abort = new AbortController();
    control.current = abort;
    setBusy('upload');
    onNote(null);
    setSending({ kind, loaded: 0, total: picked.size });
    try {
      const uploaded = await uploadWithProgress<Image>(
        `${BASE}/images`,
        picked,
        (progress) => setSending({ kind, loaded: progress.loaded, total: progress.total || picked.size }),
        abort.signal,
      );
      onChange(uploaded);
    } catch (err) {
      if (!(err instanceof DOMException && err.name === 'AbortError')) {
        onNote({ text: failText(err, kind === 'video' ? 'Видео не загрузилось' : 'Картинка не загрузилась'), tone: 'bad' });
      }
    } finally {
      control.current = null;
      setSending(null);
      setBusy(null);
    }
  };

  const drop = (event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    setOver(false);
    void start(event.dataTransfer.files[0]);
  };

  let body: ReactNode;
  if (sending) {
    body = <Uploading sending={sending} onCancel={() => control.current?.abort()} />;
  } else if (image) {
    body = <Attached image={image} busy={busy} onReplace={() => file.current?.click()} onRemove={() => onChange(null)} />;
  } else {
    body = (
      <span className="bc-drop-line">
        <button type="button" className="btn-line hov bc-upload" onClick={() => file.current?.click()} disabled={busy !== null}>
          <Icon name="paperclip" size={18} />
          &nbsp;{bot ? 'Загрузить файл' : 'Загрузить картинку'}
        </button>
        <span className="cap">или перетащите его сюда</span>
      </span>
    );
  }

  return (
    <section
      className={`sec bc-media ${over ? 'over' : ''}`}
      aria-labelledby="bc-image-h"
      onDragOver={(event) => {
        event.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={drop}
    >
      <div className="sec-head">
        <h2 id="bc-image-h" className="section-title">
          Картинка или видео
        </h2>
        <span className="muted">
          Необязательно. Картинка JPEG или PNG до {wholeMb(limits.image_bytes)}, видео MP4 до {wholeMb(limits.video_bytes)}. Текст к ним — подпись, до {count(limits.caption)}{' '}
          знаков.
        </span>
      </div>
      <div className="row">
        <div className="rc">
          <input
            ref={file}
            className="sr-only"
            type="file"
            accept={bot ? ACCEPT_ANY : ACCEPT_PHOTO}
            id="bc-file"
            tabIndex={-1}
            onChange={(e) => {
              void start(e.target.files?.[0]);
              e.target.value = '';
            }}
          />
          {body}
          <ChannelNote channel={channel} video={isVideo(image) || sending?.kind === 'video'} />
        </div>
      </div>
    </section>
  );
}
