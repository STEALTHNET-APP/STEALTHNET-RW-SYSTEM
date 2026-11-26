import { useEffect, useState, type ReactNode } from 'react';
import { Link, Navigate, Outlet, useLocation } from 'react-router';
import { api } from '../../lib/api';
import type { Permission } from '../../lib/types';
import { useSession } from '../session';
import { shortName, UPDATES_PATH, upgradeText, versionLine, type BuildSummary } from '../version';
import { firstSection, pageNeed, SECTIONS } from './settingsAccess';
import '../settings.css';
import './updates.css';

const NEED_TITLE: Record<Permission, string> = {
  clients_read: 'Клиенты',
  clients_write: 'Клиенты: изменения',
  money: 'Деньги',
  refunds: 'Возвраты',
  tariffs: 'Тарифы',
  infrastructure: 'Инфраструктура',
  support: 'Поддержка',
  marketing: 'Маркетинг',
  modules: 'Модули',
  settings: 'Настройки',
  admins: 'Администраторы',
  clients_login: 'Вход за клиента',
  remna_sync: 'Синхронизация с Remnawave',
  backups: 'Резервные копии',
  audit_log: 'Журнал действий',
};

export function needText(need: Permission[]): string {
  const names = need.map((p) => `«${NEED_TITLE[p]}»`);
  return names.length > 1 ? `с правами ${names.join(' и ')}` : `с правом ${names[0]}`;
}

export function SettingsGate() {
  const { pathname } = useLocation();
  const { can } = useSession();
  const need = pageNeed(pathname);
  if (need.every(can)) {
    return <Outlet />;
  }
  const open = firstSection(can);
  if (pathname.replace(/\/+$/, '') === '/admin/settings' && open) {
    return <Navigate to={open.to} replace />;
  }
  const text = `Раздел доступен администраторам ${needText(need)}. Если он нужен, попросите владельца поменять вам роль или права в «Настройки → Администраторы».`;
  if (!open) {
    return (
      <>
        <h1 className="page-title">Настройки</h1>
        <section className="card card-pad" role="alert">
          <p className="muted">{text}</p>
        </section>
      </>
    );
  }
  return (
    <SettingsLayout>
      <section className="sec card-pad" role="alert">
        <h2 className="section-title">Нет доступа</h2>
        <p className="muted">{text}</p>
      </section>
    </SettingsLayout>
  );
}

export function SettingsLayout({ children }: { children: ReactNode }) {
  const { pathname } = useLocation();
  const { can } = useSession();
  const settingsOk = can('settings');
  const [summary, setSummary] = useState<BuildSummary | null>(null);
  useEffect(() => {
    if (!settingsOk) {
      return;
    }
    let alive = true;
    api<BuildSummary>('/api/admin/system/summary')
      .then((s) => alive && setSummary(s))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [settingsOk]);
  const fresh = summary?.update ? shortName(summary.update.label, summary.update.commit, summary.update.built_at) : null;
  const upgrade = summary ? upgradeText(summary) : null;
  return (
    <>
      <h1 className="page-title tight">Настройки</h1>
      <div className="setgrid">
        <nav aria-label="Разделы настроек" className="subnav">
          {SECTIONS.filter((section) => section.need.every(can)).map((section) => (
            <Link
              key={section.label}
              to={section.to}
              className={section.to === UPDATES_PATH && fresh ? 'hov subnav-upd' : 'hov'}
              aria-current={pathname === section.to || pathname.startsWith(`${section.to}/`) ? 'page' : undefined}
              aria-label={section.to === UPDATES_PATH && fresh ? `${section.label}, вышла новая версия ${fresh}` : undefined}
            >
              <span className="subnav-label">{section.label}</span>
              {section.to === UPDATES_PATH && fresh ? <span className="subnav-new">новая версия {fresh}</span> : null}
            </Link>
          ))}
          {summary ? (
            <Link to={UPDATES_PATH} className={upgrade ? 'subnav-version ver-new' : 'subnav-version'}>
              {upgrade ? `STEALTHNET ${upgrade}` : versionLine('STEALTHNET', summary)}
            </Link>
          ) : null}
        </nav>
        <div className="setbody">{children}</div>
      </div>
    </>
  );
}
