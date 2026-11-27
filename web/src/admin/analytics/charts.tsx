import { useCallback, useEffect, useRef, useState } from 'react';
import type { Currency } from '../../lib/types';
import { axisLabel, format, type Point, type Step, type Unit } from './model';

export function useWidth<T extends HTMLElement>(): [(node: T | null) => void, number] {
  const [width, setWidth] = useState(0);
  const observer = useRef<ResizeObserver | null>(null);
  const ref = useCallback((node: T | null) => {
    observer.current?.disconnect();
    if (!node) {
      return;
    }
    setWidth(node.clientWidth);
    observer.current = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box) {
        setWidth(Math.round(box.width));
      }
    });
    observer.current.observe(node);
  }, []);
  useEffect(() => () => observer.current?.disconnect(), []);
  return [ref, width];
}

function niceStep(max: number, lines: number): number {
  const raw = max / lines;
  const power = 10 ** Math.floor(Math.log10(Math.max(raw, 1)));
  for (const k of [1, 2, 2.5, 5, 10]) {
    if (k * power >= raw) {
      return k * power;
    }
  }
  return 10 * power;
}

type Props = {
  points: Point[];
  prev: Point[];
  unit: Unit;
  currency: Currency;
  step: Step;
  live: boolean;
  height: number;
  active: number | null;
  onActive: (index: number | null) => void;
  compact?: boolean;
};

export function SeriesChart({ points, prev, unit, currency, step, live, height, active, onActive, compact = false }: Props) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const bottom = 24;
  const top = 8;
  const plot = height - bottom - top;
  const values = [...points.map((p) => p.value), ...prev.map((p) => p.value)];
  const max = Math.max(1, ...values);
  const min = Math.min(0, ...values);
  const span = max - min;
  const stepY = niceStep(span || 1, compact ? 3 : 4);
  const ceil = Math.ceil(max / stepY) * stepY;
  const floor = Math.floor(min / stepY) * stepY;
  const lines: number[] = [];
  for (let v = floor; v <= ceil + stepY / 2; v += stepY) {
    lines.push(v);
  }
  const longest = Math.max(1, ...lines.map((v) => (v === 0 ? 1 : format(v, unit, currency, true).length)));
  const left = Math.max(compact ? 34 : 56, Math.round(longest * (compact ? 6.4 : 7)) + 12);
  const inner = Math.max(0, width - left - 4);
  const n = Math.max(points.length, prev.length, 1);
  const slot = inner / n;
  const bar = Math.max(2, Math.min(36, slot * (n > 40 ? 0.66 : 0.56)));
  const y = (v: number) => top + plot - ((v - floor) / (ceil - floor || 1)) * plot;
  const x = (i: number) => left + slot * i + slot / 2;
  const every = Math.max(1, Math.ceil((compact ? 44 : 52) / Math.max(slot, 1)));
  const single = prev.length === 1 ? prev[0] : undefined;
  const path = single
    ? `M${(x(0) - bar / 2 - 10).toFixed(1)},${y(single.value).toFixed(1)} L${(x(0) + bar / 2 + 10).toFixed(1)},${y(single.value).toFixed(1)}`
    : prev.length
      ? prev.map((p, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(p.value).toFixed(1)}`).join(' ')
      : '';
  return (
    <div className="an-chart" ref={ref} style={{ height }} onMouseLeave={() => onActive(null)}>
      {width > 0 ? (
        <svg width={width} height={height} role="img" aria-label="График показателя">
          {lines.map((v) => (
            <g key={v}>
              <line x1={left} x2={width} y1={y(v)} y2={y(v)} className={v === 0 ? 'an-grid strong' : 'an-grid'} />
              <text x={left - 8} y={y(v) + 4} textAnchor="end" className="an-axis">
                {v === 0 ? '0' : format(v, unit, currency, true)}
              </text>
            </g>
          ))}
          {points.map((p, i) => {
            const top0 = y(Math.max(0, p.value));
            const h = Math.max(p.value !== 0 ? 2 : 0, Math.abs(y(p.value) - y(0)));
            return (
              <rect
                key={p.at}
                x={x(i) - bar / 2}
                y={p.value >= 0 ? top0 : y(0)}
                width={bar}
                height={h}
                rx={Math.min(3, bar / 3)}
                className={`an-bar${i === active ? ' on' : ''}${live && i === points.length - 1 ? ' live' : ''}`}
              />
            );
          })}
          {path ? <path d={path} className="ax-prev" /> : null}
          {points.map((p, i) =>
            i % every === 0 ? (
              <text key={`l${p.at}`} x={x(i)} y={height - 6} textAnchor="middle" className={`an-axis${i === active ? ' on' : ''}`}>
                {axisLabel(p.at, step, i === 0, i > 0 ? (points[i - every]?.at ?? null) : null)}
              </text>
            ) : null,
          )}
          {points.map((p, i) => (
            <rect
              key={`h${p.at}`}
              x={left + slot * i}
              y={top}
              width={slot}
              height={plot}
              fill="transparent"
              onMouseEnter={() => onActive(i)}
              onClick={() => onActive(i)}
            />
          ))}
        </svg>
      ) : null}
    </div>
  );
}

export function ShareBar({ value }: { value: number }) {
  return (
    <span className="an-share">
      <span style={{ width: `${Math.max(0, Math.min(100, value))}%` }} />
    </span>
  );
}
