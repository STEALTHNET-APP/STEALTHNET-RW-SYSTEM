import { Fragment, useId, useLayoutEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from 'react';
import { t } from './i18n';
import { firstParagraph, parseTg, type TgNode } from './tgMarkup';
import './tgText.css';

type LinkHandler = (href: string) => void;

function renderTg(nodes: TgNode[], prefix: string, onLink?: LinkHandler): ReactNode[] {
  return nodes.map((node, index) => {
    const key = `${prefix}.${index}`;
    if (typeof node === 'string') {
      return <Fragment key={key}>{node}</Fragment>;
    }
    const inner = renderTg(node.children, key, onLink);
    switch (node.tag) {
      case 'b':
        return <strong key={key}>{inner}</strong>;
      case 'i':
        return <em key={key}>{inner}</em>;
      case 'u':
        return <u key={key}>{inner}</u>;
      case 's':
        return <s key={key}>{inner}</s>;
      case 'code':
        return <code key={key}>{inner}</code>;
      case 'pre':
        return (
          <code key={key} className="tgh-pre">
            {inner}
          </code>
        );
      case 'quote':
        return (
          <span key={key} className="tgh-quote">
            {inner}
          </span>
        );
      case 'a': {
        const href = node.href ?? '';
        const click = onLink
          ? (event: MouseEvent<HTMLAnchorElement>) => {
              event.preventDefault();
              onLink(href);
            }
          : undefined;
        return (
          <a key={key} href={href} target="_blank" rel="noopener noreferrer nofollow" onClick={click}>
            {inner}
          </a>
        );
      }
      default:
        return <Fragment key={key}>{inner}</Fragment>;
    }
  });
}

type TextProps = { html: string; className?: string; onLink?: LinkHandler };

export function TgText({ html, className, onLink }: TextProps) {
  const nodes = useMemo(() => parseTg(html).nodes, [html]);
  return <div className={className ? `tgh ${className}` : 'tgh'}>{renderTg(nodes, 'n', onLink)}</div>;
}

type NoteProps = TextProps & { toggleClass?: string };

export function TariffNote({ html, className, onLink, toggleClass }: NoteProps) {
  const nodes = useMemo(() => parseTg(html.trim()).nodes, [html]);
  const head = useMemo(() => firstParagraph(nodes), [nodes]);
  const box = useRef<HTMLDivElement | null>(null);
  const id = useId();
  const [open, setOpen] = useState(false);
  const [long, setLong] = useState(false);

  useLayoutEffect(() => {
    setOpen(false);
  }, [html]);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el || open) {
      return;
    }
    const check = () => setLong(el.scrollHeight - el.clientHeight > 2);
    check();
    if (typeof ResizeObserver === 'undefined') {
      return;
    }
    const watch = new ResizeObserver(check);
    watch.observe(el);
    return () => watch.disconnect();
  }, [head, open]);

  if (!nodes.length) {
    return null;
  }
  return (
    <div className={className ? `tgh-note ${className}` : 'tgh-note'}>
      <div ref={box} id={id} className={open ? 'tgh' : 'tgh tgh-clamp'}>
        {renderTg(open ? nodes : head.nodes, 'n', onLink)}
      </div>
      {long || head.more ? (
        <button type="button" className={toggleClass ? `tgh-more ${toggleClass}` : 'tgh-more'} aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
          {open ? t('Свернуть') : t('Показать полностью')}
        </button>
      ) : null}
    </div>
  );
}
