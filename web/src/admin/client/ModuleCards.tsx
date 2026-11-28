import { useEffect, useRef, useState } from 'react';
import { api } from '../../lib/api';
import { useSession } from '../session';
import { CHANGED_MESSAGE, FRAME_SANDBOX, frameHeight, HEIGHT_MESSAGE, THEME_MESSAGE, withTheme, type CardFrame } from '../modules/frame';

function ModuleFrame({ frame, onChanged }: { frame: CardFrame; onChanged: () => void }) {
  const { theme } = useSession();
  const ref = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(120);
  const [src] = useState(() => withTheme(frame.url, theme));
  const changed = useRef(onChanged);
  changed.current = onChanged;

  useEffect(() => {
    const listener = (event: MessageEvent) => {
      const target = ref.current?.contentWindow;
      if (!target || event.source !== target || event.origin !== 'null') {
        return;
      }
      const data = event.data as { type?: unknown; height?: unknown } | null;
      if (data?.type === HEIGHT_MESSAGE) {
        const next = frameHeight(data.height);
        if (next !== null) {
          setHeight(next);
        }
      } else if (data?.type === CHANGED_MESSAGE) {
        changed.current();
      }
    };
    window.addEventListener('message', listener);
    return () => window.removeEventListener('message', listener);
  }, []);

  useEffect(() => {
    ref.current?.contentWindow?.postMessage({ type: THEME_MESSAGE, theme }, '*');
  }, [theme]);

  return <iframe ref={ref} className="cc-modframe" title={frame.title} src={src} sandbox={FRAME_SANDBOX} referrerPolicy="no-referrer" style={{ height }} />;
}

export function ModuleCards({ clientId, phone = false, onChanged }: { clientId: number; phone?: boolean; onChanged: () => void }) {
  const [frames, setFrames] = useState<CardFrame[]>([]);

  useEffect(() => {
    let alive = true;
    api<CardFrame[]>(`/api/admin/clients/${clientId}/module-cards`)
      .then((list) => {
        if (alive) {
          setFrames(list);
        }
      })
      .catch(() => {
        if (alive) {
          setFrames([]);
        }
      });
    return () => {
      alive = false;
    };
  }, [clientId]);

  if (phone) {
    return (
      <>
        {frames.map((frame) => (
          <section key={frame.module} className="pc-group" aria-label={frame.title}>
            <h2 className="hp-h2">{frame.title}</h2>
            <div className="pc-card pc-modcard">
              <ModuleFrame frame={frame} onChanged={onChanged} />
            </div>
          </section>
        ))}
      </>
    );
  }

  return (
    <>
      {frames.map((frame) => (
        <section key={frame.module} className="cc-sec cc-list cc-modsec" aria-label={frame.title}>
          <h2 className="cc-h2">{frame.title}</h2>
          <ModuleFrame frame={frame} onChanged={onChanged} />
        </section>
      ))}
    </>
  );
}
