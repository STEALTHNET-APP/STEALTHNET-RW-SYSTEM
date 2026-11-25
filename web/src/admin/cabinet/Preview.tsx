import { useEffect, useRef, useState } from 'react';
import type { Lang } from '../../lib/i18n';
import type { Theme } from '../../lib/theme';
import type { Site } from '../../lib/types';
import { isPart, type Part } from '../../cabinet/parts';
import { CABINET_PREVIEW, type CabinetDraft, type PreviewScreen, type PreviewState } from '../../cabinet/preview/model';
import { PreviewFail, usePreviewFrame } from '../previewFrame';
import { useSize } from '../miniapp/Preview';

export const PREVIEW_PATH = '/admin/cabinet-preview';

export type Device = 'desktop' | 'phone';
export type Focus = { part: Part; n: number };

type Props = { site: Site; css: string; device: Device; theme: Theme; lang: Lang; state: PreviewState; screen: PreviewScreen; focus: Focus | null; height: number; onPick: (part: Part) => void };

export function PreviewFrame({ site, css, device, theme, lang, state, screen, focus, height, onPick }: Props) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const { width } = useSize(box);
  const virtual = device === 'desktop' ? 1280 : 390;
  const scale = width > 0 ? Math.min(1, width / virtual) : 0;
  const { frame, ready, attempt, problem, retry, send } = usePreviewFrame(PREVIEW_PATH, CABINET_PREVIEW.ready, scale > 0, (data) => {
    if (data.type === CABINET_PREVIEW.pick && isPart(data.part)) {
      onPick(data.part);
    }
  });
  const sent = useRef(0);

  useEffect(() => {
    if (!ready) {
      return;
    }
    const timer = window.setTimeout(() => {
      const jump = focus && focus.n !== sent.current ? focus.part : null;
      if (focus) {
        sent.current = focus.n;
      }
      const message: CabinetDraft = { type: CABINET_PREVIEW.draft, site, css, theme, lang, state, screen, focus: jump };
      send(message);
    }, 120);
    return () => window.clearTimeout(timer);
  }, [ready, send, site, css, theme, lang, state, screen, focus]);

  return (
    <div ref={setBox} className={`cbe-frame ${device}`} style={{ height }}>
      {scale > 0 ? (
        <div className="cbe-frame-in" style={{ width: Math.floor(virtual * scale), height }}>
          <iframe key={attempt} ref={frame} src={PREVIEW_PATH} title="Превью кабинета" style={{ width: virtual, height: Math.ceil(height / scale), transform: `scale(${scale})` }} />
        </div>
      ) : null}
      <PreviewFail problem={problem} onRetry={retry} />
    </div>
  );
}
