import { escapeTg, parseTg, safeLink, tgToHtml, type TgNode, type TgTag } from '../../lib/tgMarkup';

const ELEMENTS: Record<TgTag, string> = { b: 'b', i: 'i', u: 'u', s: 's', a: 'a', code: 'code', pre: 'pre', quote: 'blockquote', spoiler: 'span', emoji: 'span' };

const BLOCKS = new Set(['address', 'article', 'aside', 'dd', 'div', 'dl', 'dt', 'footer', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'header', 'li', 'main', 'nav', 'ol', 'p', 'section', 'table', 'tr', 'ul']);

const SKIPPED = new Set(['head', 'meta', 'script', 'style', 'template', 'title', 'noscript', 'svg', 'math', 'iframe', 'object']);

function appendTg(parent: Node, nodes: TgNode[]): void {
  const doc = parent.ownerDocument ?? document;
  for (const node of nodes) {
    if (typeof node === 'string') {
      node.split('\n').forEach((part, index) => {
        if (index > 0) {
          parent.appendChild(doc.createElement('br'));
        }
        if (part) {
          parent.appendChild(doc.createTextNode(part));
        }
      });
      continue;
    }
    const el = doc.createElement(ELEMENTS[node.tag]);
    if (node.tag === 'a') {
      el.setAttribute('href', node.href ?? '');
    } else if (node.tag === 'spoiler') {
      el.dataset.tg = 'spoiler';
      el.className = 'tf-spoiler';
    } else if (node.tag === 'emoji') {
      el.dataset.emoji = node.id ?? '';
    }
    if (node.extra) {
      el.dataset.extra = node.extra;
    }
    appendTg(el, node.children);
    parent.appendChild(el);
  }
}

export function fillEditor(root: HTMLElement, html: string): void {
  root.replaceChildren();
  appendTg(root, parseTg(html).nodes);
  if (!root.lastChild || root.lastChild.nodeName === 'BR') {
    root.appendChild(root.ownerDocument.createElement('br'));
  }
}

export function tgFragment(html: string): string {
  const box = document.createElement('div');
  appendTg(box, parseTg(html).nodes);
  return box.innerHTML;
}

function weight(el: HTMLElement): boolean | null {
  const raw = el.style.fontWeight;
  if (!raw) {
    return null;
  }
  return raw === 'bold' || raw === 'bolder' || Number(raw) >= 600;
}

function opening(el: HTMLElement): string[] {
  const name = el.localName;
  const found: string[] = [];
  const bold = weight(el);
  if (bold === true || ((name === 'b' || name === 'strong') && bold === null)) {
    found.push('<b>');
  }
  if (name === 'i' || name === 'em' || el.style.fontStyle === 'italic') {
    found.push('<i>');
  }
  const decoration = `${el.style.textDecoration} ${el.style.textDecorationLine}`;
  if (name === 'u' || name === 'ins' || decoration.includes('underline')) {
    found.push('<u>');
  }
  if (name === 's' || name === 'strike' || name === 'del' || decoration.includes('line-through')) {
    found.push('<s>');
  }
  if (name === 'code' || name === 'kbd' || name === 'samp' || name === 'tt') {
    const lang = el.dataset.extra ?? el.getAttribute('class') ?? '';
    found.push(/^language-[\w+#.-]{1,30}$/.test(lang) ? `<code class="${lang}">` : '<code>');
  }
  if (name === 'pre') {
    found.push('<pre>');
  }
  if (name === 'blockquote') {
    found.push(el.dataset.extra === 'expandable' || el.hasAttribute('expandable') ? '<blockquote expandable>' : '<blockquote>');
  }
  if (el.dataset.tg === 'spoiler' || name === 'tg-spoiler' || el.classList.contains('tg-spoiler')) {
    found.push('<tg-spoiler>');
  }
  if (name === 'a') {
    const href = safeLink(el.getAttribute('href'));
    if (href) {
      found.unshift(`<a href="${escapeTg(href).replace(/"/g, '&quot;')}">`);
    }
  }
  return found;
}

function closing(tag: string): string {
  return `</${/^<([a-z-]+)/.exec(tag)?.[1] ?? 'b'}>`;
}

function keeps(el: HTMLElement): boolean {
  return el.localName === 'pre' || /^pre/.test(el.style.whiteSpace);
}

export function editorHtml(root: Node, editor = true): string {
  let out = '';
  let line = '';
  let pending = false;
  const push = (value: string) => {
    out += escapeTg(value);
    const cut = value.lastIndexOf('\n');
    line = cut < 0 ? line + value : value.slice(cut + 1);
  };
  const settle = () => {
    if (pending && line !== '') {
      push('\n');
    }
    pending = false;
  };
  const walk = (node: Node, keep: boolean) => {
    const children = Array.from(node.childNodes);
    children.forEach((child, index) => {
      if (child.nodeType === Node.TEXT_NODE) {
        const raw = (child.textContent ?? '').replace(/ /g, ' ');
        if (keep) {
          if (raw) {
            settle();
            push(raw.replace(/\r\n?/g, '\n'));
          }
          return;
        }
        const value = raw.replace(/\s+/g, ' ');
        if (!value.trim()) {
          if (value && !pending && line !== '' && !line.endsWith(' ')) {
            push(' ');
          }
          return;
        }
        settle();
        push(line === '' || line.endsWith(' ') ? value.replace(/^ /, '') : value);
        return;
      }
      if (child.nodeType !== Node.ELEMENT_NODE) {
        return;
      }
      const el = child as HTMLElement;
      const name = el.localName;
      if (SKIPPED.has(name)) {
        return;
      }
      if (name === 'br') {
        const parent = node as Element;
        if (index === children.length - 1 && node !== root && BLOCKS.has(parent.localName ?? '')) {
          return;
        }
        settle();
        push('\n');
        return;
      }
      if (name === 'img') {
        const alt = el.getAttribute('alt') ?? '';
        if (alt) {
          settle();
          push(alt);
        }
        return;
      }
      const emoji = el.dataset.emoji ?? (name === 'tg-emoji' ? el.getAttribute('emoji-id') : null);
      if (emoji && /^\d{1,24}$/.test(emoji)) {
        settle();
        out += `<tg-emoji emoji-id="${emoji}">`;
        walk(el, keep);
        out += '</tg-emoji>';
        return;
      }
      if (BLOCKS.has(name)) {
        pending = true;
        settle();
        walk(el, keep || keeps(el));
        pending = true;
        return;
      }
      const tags = opening(el);
      out += tags.join('');
      walk(el, keep || keeps(el));
      out += tags
        .slice()
        .reverse()
        .map(closing)
        .join('');
    });
  };
  walk(root, editor);
  return tgToHtml(parseTg(out).nodes);
}

export function pastedHtml(clean: string, plain: string): string {
  if (!clean.trim()) {
    return escapeTg(plain.replace(/\r\n?/g, '\n'));
  }
  const doc = new DOMParser().parseFromString(clean, 'text/html');
  return editorHtml(doc.body, false).replace(/^\n+|\n+$/g, '');
}

export function linkAt(node: Node | null, root: HTMLElement): HTMLAnchorElement | null {
  let current: Node | null = node;
  while (current && current !== root) {
    if (current.nodeType === Node.ELEMENT_NODE && (current as Element).localName === 'a') {
      return current as HTMLAnchorElement;
    }
    current = current.parentNode;
  }
  return null;
}
