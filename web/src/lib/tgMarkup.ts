export type TgTag = 'b' | 'i' | 'u' | 's' | 'a' | 'code' | 'pre' | 'quote' | 'spoiler' | 'emoji';

export type TgElement = { tag: TgTag; href?: string; id?: string; extra?: string; children: TgNode[] };

export type TgNode = string | TgElement;

const NAMES: Record<string, TgTag> = {
  b: 'b',
  strong: 'b',
  i: 'i',
  em: 'i',
  u: 'u',
  ins: 'u',
  s: 's',
  strike: 's',
  del: 's',
  a: 'a',
  code: 'code',
  pre: 'pre',
  blockquote: 'quote',
  'tg-spoiler': 'spoiler',
  span: 'spoiler',
  'tg-emoji': 'emoji',
};

const ENTITY = /^&(amp|lt|gt|quot|#\d{1,7}|#[xX][0-9a-fA-F]{1,6});/;

function entity(raw: string): string {
  const name = raw.slice(1, -1);
  if (name === 'amp') {
    return '&';
  }
  if (name === 'lt') {
    return '<';
  }
  if (name === 'gt') {
    return '>';
  }
  if (name === 'quot') {
    return '"';
  }
  const code = name[1] === 'x' || name[1] === 'X' ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
  return Number.isInteger(code) && code > 0 && code <= 0x10ffff && (code < 0xd800 || code > 0xdfff) ? String.fromCodePoint(code) : '';
}

function attribute(raw: string, name: string): string | null {
  const at = raw.indexOf(`${name}=`);
  if (at < 0) {
    return null;
  }
  const quote = raw[at + name.length + 1];
  if (quote !== '"' && quote !== "'") {
    return null;
  }
  const start = at + name.length + 2;
  const end = raw.indexOf(quote, start);
  return end < 0 ? null : raw.slice(start, end);
}

function decode(text: string): string {
  let out = '';
  let rest = text;
  let at = rest.indexOf('&');
  while (at >= 0) {
    out += rest.slice(0, at);
    const tail = rest.slice(at);
    const found = ENTITY.exec(tail);
    if (found) {
      out += entity(found[0]);
      rest = tail.slice(found[0].length);
    } else {
      out += '&';
      rest = tail.slice(1);
    }
    at = rest.indexOf('&');
  }
  return out + rest;
}

export function safeLink(href: string | null): string | null {
  if (!href) {
    return null;
  }
  const value = decode(href).trim();
  return /^(https?:\/\/|tg:\/\/)[^\s"'<>]+$/i.test(value) ? value : null;
}

type Open = { name: string; node: TgElement | null; children: TgNode[] };

export type TgProblem = { kind: 'unclosed' | 'unexpected' | 'link' | 'span' | 'emoji'; tag: string };

export function problemText(problem: TgProblem): string {
  switch (problem.kind) {
    case 'unclosed':
      return `тег <${problem.tag}> не закрыт`;
    case 'unexpected':
      return `лишний закрывающий тег </${problem.tag}>`;
    case 'link':
      return 'у ссылки должен быть адрес вида <a href="https://…">текст</a>';
    case 'span':
      return 'у <span> допустим только class="tg-spoiler"';
    default:
      return 'премиум-эмодзи пишется так: <tg-emoji emoji-id="5368324170671202286">🔥</tg-emoji>';
  }
}

function blocked(stack: Open[], name: string): boolean {
  const inside = (tag: string) => stack.some((o) => o.node !== null && o.name === tag);
  if (inside('code') || (inside('pre') && name !== 'code')) {
    return true;
  }
  if (name === 'a' && inside('a')) {
    return true;
  }
  return name === 'blockquote' && inside('blockquote');
}

export function parseTg(html: string): { nodes: TgNode[]; problem: TgProblem | null } {
  const root: Open = { name: '', node: null, children: [] };
  const stack: Open[] = [root];
  let problem: TgProblem | null = null;
  const note = (next: TgProblem) => {
    problem ??= next;
  };
  const top = () => stack[stack.length - 1] ?? root;
  const text = (value: string) => {
    if (!value) {
      return;
    }
    const list = top().children;
    const last = list[list.length - 1];
    if (typeof last === 'string') {
      list[list.length - 1] = last + value;
    } else {
      list.push(value);
    }
  };
  const close = () => {
    const done = stack.pop();
    if (!done) {
      return;
    }
    if (done.node) {
      done.node.children = done.children;
      top().children.push(done.node);
    } else {
      for (const child of done.children) {
        if (typeof child === 'string') {
          text(child);
        } else {
          top().children.push(child);
        }
      }
    }
  };
  let rest = html.replace(/\r\n?/g, '\n');
  while (rest) {
    const at = rest.search(/[<&]/);
    if (at < 0) {
      text(rest);
      break;
    }
    text(rest.slice(0, at));
    const tail = rest.slice(at);
    if (tail[0] === '&') {
      const found = ENTITY.exec(tail);
      text(found ? entity(found[0]) : '&');
      rest = tail.slice(found ? found[0].length : 1);
      continue;
    }
    const end = tail.indexOf('>');
    const inner = end < 0 ? '' : tail.slice(1, end);
    const closing = inner.startsWith('/');
    const body = closing ? inner.slice(1) : inner;
    const name = (/^[A-Za-z0-9-]*/.exec(body)?.[0] ?? '').toLowerCase();
    if (end < 0 || !name || !(name in NAMES) || inner.includes('<')) {
      text('<');
      rest = tail.slice(1);
      continue;
    }
    rest = tail.slice(end + 1);
    if (closing) {
      let index = -1;
      for (let i = stack.length - 1; i > 0; i -= 1) {
        if (stack[i]?.name === name) {
          index = i;
          break;
        }
      }
      if (index < 0) {
        note({ kind: 'unexpected', tag: name });
        continue;
      }
      if (index !== stack.length - 1) {
        note({ kind: 'unexpected', tag: name });
      }
      while (stack.length > index) {
        close();
      }
      continue;
    }
    const attrs = body.slice(name.length);
    const tag = NAMES[name] as TgTag;
    let node: Open['node'] = { tag, children: [] };
    if (name === 'a') {
      const href = attribute(attrs, 'href');
      const link = safeLink(href);
      if (!link) {
        note({ kind: 'link', tag: name });
        node = null;
      } else {
        node.href = link;
      }
    } else if (name === 'span' && attribute(attrs, 'class') !== 'tg-spoiler') {
      note({ kind: 'span', tag: name });
      node = null;
    } else if (name === 'tg-emoji') {
      const id = attribute(attrs, 'emoji-id') ?? '';
      if (/^\d{1,24}$/.test(id)) {
        node.id = id;
      } else {
        note({ kind: 'emoji', tag: name });
        node = null;
      }
    } else if (name === 'blockquote' && /(^|\s)expandable(\s|=|$)/.test(attrs)) {
      node.extra = 'expandable';
    } else if (name === 'code') {
      const lang = attribute(attrs, 'class');
      if (lang && /^language-[\w+#.-]{1,30}$/.test(lang)) {
        node.extra = lang;
      }
    }
    if (node && blocked(stack, name)) {
      node = null;
    }
    stack.push({ name, node, children: [] });
  }
  if (stack.length > 1) {
    note({ kind: 'unclosed', tag: stack[stack.length - 1]?.name ?? '' });
  }
  while (stack.length > 1) {
    close();
  }
  return { nodes: root.children, problem };
}

export function tgPlain(html: string): string {
  return plainOf(parseTg(html).nodes);
}

export function tgLine(html: string): string {
  return tgPlain(html).replace(/\s+/g, ' ').trim();
}

export function tgLength(html: string): number {
  return tgPlain(html).length;
}

const OUT: Record<TgTag, string> = { b: 'b', i: 'i', u: 'u', s: 's', a: 'a', code: 'code', pre: 'pre', quote: 'blockquote', spoiler: 'tg-spoiler', emoji: 'tg-emoji' };

export function escapeTg(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function attrsOf(node: TgElement): string {
  switch (node.tag) {
    case 'a':
      return ` href="${escapeTg(node.href ?? '').replace(/"/g, '&quot;')}"`;
    case 'emoji':
      return ` emoji-id="${node.id ?? ''}"`;
    case 'quote':
      return node.extra ? ' expandable' : '';
    case 'code':
      return node.extra ? ` class="${node.extra}"` : '';
    default:
      return '';
  }
}

export function tgToHtml(nodes: TgNode[]): string {
  return nodes
    .map((node) => {
      if (typeof node === 'string') {
        return escapeTg(node);
      }
      const name = OUT[node.tag];
      return `<${name}${attrsOf(node)}>${tgToHtml(node.children)}</${name}>`;
    })
    .join('');
}

function plainOf(nodes: TgNode[]): string {
  return nodes.map((node) => (typeof node === 'string' ? node : plainOf(node.children))).join('');
}

export function cutTg(nodes: TgNode[], at: number): TgNode[] {
  let left = at;
  const walk = (list: TgNode[]): TgNode[] => {
    const out: TgNode[] = [];
    for (const node of list) {
      if (left <= 0) {
        break;
      }
      if (typeof node === 'string') {
        out.push(node.slice(0, left));
        left -= Math.min(left, node.length);
      } else {
        out.push({ ...node, children: walk(node.children) });
      }
    }
    return out;
  };
  return walk(nodes);
}

export function firstParagraph(nodes: TgNode[]): { nodes: TgNode[]; more: boolean } {
  const plain = plainOf(nodes);
  const lead = plain.length - plain.trimStart().length;
  const gap = /\n[^\S\n]*\n/.exec(plain.slice(lead));
  if (!gap || !plain.slice(lead + gap.index).trim()) {
    return { nodes, more: false };
  }
  return { nodes: cutTg(nodes, lead + gap.index), more: true };
}
