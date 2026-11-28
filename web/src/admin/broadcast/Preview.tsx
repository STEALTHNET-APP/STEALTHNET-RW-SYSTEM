import { useState } from 'react';
import { Icon } from '../../lib/icons';
import { TelegramText } from '../pages/telegram';
import { clock } from './files';
import { imageUrl, isVideo, type ButtonSpec, type Channel, type Image } from './model';
import { premiumCount } from './emoji';
import '../pages/bot.css';

type Props = {
  text: string;
  image: Image | null;
  buttons: ButtonSpec[];
  className?: string;
};

function Clip({ image }: { image: Image }) {
  const [playing, setPlaying] = useState(false);
  const src = imageUrl(image.id);
  if (playing) {
    return <video src={src} className="bt-preview-img bt-clip" controls autoPlay playsInline aria-label="Видео сообщения" />;
  }
  return (
    <button type="button" className="bt-clip" onClick={() => setPlaying(true)} aria-label="Посмотреть видео">
      <video src={`${src}#t=0.1`} className="bt-preview-img" preload="metadata" muted playsInline tabIndex={-1} />
      <span className="bt-clip-time">{clock(image.duration)}</span>
      <span className="bt-clip-play" aria-hidden="true" />
    </button>
  );
}

export function Preview({ text, image, buttons, className }: Props) {
  return (
    <div className={`bc-preview ${className ?? ''}`} aria-label="Как увидит клиент">
      <span className="bc-preview-cap">Так увидит клиент</span>
      <div className="bt-preview">
        {image && isVideo(image) ? <Clip key={image.id} image={image} /> : image ? <img src={imageUrl(image.id)} alt="" className="bt-preview-img" /> : null}
        {text.trim() ? (
          <div className="bt-bubble">
            <TelegramText html={text} />
          </div>
        ) : (
          <span className="cap">Здесь появится текст сообщения</span>
        )}
      </div>
      {buttons.length ? (
        <div className="bc-keys">
          {buttons.map((button, index) => (
            <span key={index} className={`bt-tg-btn bc-key ${button.style ?? ''}`}>
              {button.emoji_id ? <span className="bt-icon" title="Значок из премиум-эмодзи" aria-hidden="true" /> : null}
              <span>{button.text.trim() || 'Кнопка'}</span>
              {button.url !== undefined && !button.section ? <Icon name="external" size={13} /> : null}
            </span>
          ))}
        </div>
      ) : null}
      {premiumCount(text) || buttons.some((button) => button.emoji_id) ? (
        <span className="cap bc-preview-note">Премиум-эмодзи здесь показаны обычными и подчёркнуты пунктиром, значок кнопки — серым квадратом.</span>
      ) : null}
    </div>
  );
}

type MailProps = {
  subject: string;
  text: string;
  image: Image | null;
  buttons: ButtonSpec[];
  brand: string;
};

export function MailPreview({ subject, text, image, buttons, brand }: MailProps) {
  return (
    <div className="bc-preview" aria-label="Как выглядит письмо">
      <span className="bc-preview-cap">Так выглядит письмо</span>
      <div className="bc-mail">
        <div className="bc-mail-head">
          <span className="bc-mail-from">{brand}</span>
          <span className="bc-mail-subject">{subject.trim() || 'Тема письма'}</span>
        </div>
        <div className="bc-mail-card">
          {image && isVideo(image) ? (
            <span className="cap bc-mail-body">Видео в письмо не попадёт: его увидят только в Telegram.</span>
          ) : image ? (
            <img src={imageUrl(image.id)} alt="" className="bc-mail-img" />
          ) : null}
          {text.trim() ? (
            <div className="bc-mail-body">
              <TelegramText html={text} />
            </div>
          ) : (
            <span className="cap bc-mail-body">Здесь появится текст письма</span>
          )}
          {buttons.length ? (
            <div className="bc-mail-keys">
              {buttons.map((button, index) => (
                <span key={index} className="bc-mail-key">
                  {button.text.trim() || 'Кнопка'}
                </span>
              ))}
            </div>
          ) : null}
        </div>
        <p className="bc-mail-foot">
          Это письмо пришло, потому что у вас есть аккаунт в {brand}. <span className="bc-mail-link">Отписаться от рассылок</span>
        </p>
      </div>
    </div>
  );
}

type ChannelProps = {
  channel: Channel;
  subject: string;
  text: string;
  image: Image | null;
  buttons: ButtonSpec[];
  brand: string;
};

export function ChannelPreview({ channel, subject, text, image, buttons, brand }: ChannelProps) {
  const [picked, setPicked] = useState<'bot' | 'email'>(channel === 'email' ? 'email' : 'bot');
  const shown = channel === 'both' ? picked : channel === 'email' ? 'email' : 'bot';
  return (
    <div className="bc-preview-wrap">
      {channel === 'both' ? (
        <span className="segs bc-preview-tabs" role="group" aria-label="Что показать">
          <button type="button" aria-pressed={shown === 'bot'} onClick={() => setPicked('bot')}>
            Telegram
          </button>
          <button type="button" aria-pressed={shown === 'email'} onClick={() => setPicked('email')}>
            Письмо
          </button>
        </span>
      ) : null}
      {shown === 'email' ? (
        <MailPreview subject={subject} text={text} image={image} buttons={buttons} brand={brand} />
      ) : (
        <Preview text={text} image={image} buttons={buttons} />
      )}
    </div>
  );
}
