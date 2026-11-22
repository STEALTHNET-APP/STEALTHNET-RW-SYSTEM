import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Link, useLocation } from 'react-router';
import { Icon } from '../lib/icons';
import { BrandMark } from '../lib/brand';
import { useMedia, usePhone } from '../lib/media';
import { readStored, writeStored, type Theme } from '../lib/theme';
import { isCurrent, navItems, type NavItem } from './nav';
import { REPORT_PATH } from './report/text';
import { ROLE_LABEL, useSession } from './session';
import { BuildChip, VersionLine, type BuildSummary } from './version';
import './shell.css';

const PIN_KEY = 'sn.admin.pinned';

type Props = {
  theme: Theme;
  onFlipTheme: () => void;
  supportBadge?: number;
  moneyBadge?: number;
  build?: BuildSummary | null;
  children: ReactNode;
};

type MenuProps = { items: NavItem[]; path: string; theme: Theme; onFlipTheme: () => void; build: BuildSummary | null };

const UPDATE_LABEL = 'вышла новая сборка';

function initial(text: string | null | undefined): string {
  const first = (text ?? '').trim().charAt(0);
  return first ? first.toUpperCase() : 'А';
}

function useOutside(open: boolean, close: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        close();
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        close();
      }
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, close]);
  return ref;
}

function ThemeLabel(theme: Theme): string {
  return theme === 'dark' ? 'Светлая тема' : 'Тёмная тема';
}

function Rail({ items, path, theme, onFlipTheme, build }: MenuProps) {
  const { me, logout, can } = useSession();
  const [pinned, setPinned] = useState(() => readStored(PIN_KEY) === '1');
  const [whoOpen, setWhoOpen] = useState(false);
  const whoRef = useOutside(whoOpen, () => setWhoOpen(false));
  const togglePin = () => {
    setPinned((value) => {
      writeStored(PIN_KEY, value ? null : '1');
      return !value;
    });
  };
  let lastGroup: NavItem['group'] | null = null;
  return (
    <div className={pinned ? 'rail-hold pinned' : 'rail-hold'}>
      <nav aria-label="Меню" className={pinned ? 'rail pinned' : 'rail'}>
        <div className="rail-logo">
          <BrandMark name={me.brand_name} logo={me.logo_url} className="mark" />
          <span className="lbl">{me.brand_name.toUpperCase()}</span>
        </div>
        {items.map((item) => {
          const sep = lastGroup !== null && item.group !== lastGroup && item.group !== 'core';
          lastGroup = item.group;
          const current = isCurrent(item, path);
          return (
            <span key={item.key} style={{ display: 'contents' }}>
              {sep ? <span className="rail-sep" aria-hidden="true" /> : null}
              <Link to={item.to} className="rail-item hov" aria-current={current ? 'page' : undefined} aria-label={item.dot ? `${item.label}, ${UPDATE_LABEL}` : undefined}>
                <span className="rail-icon">
                  <Icon name={item.icon} />
                  {item.badge ? <span className="rail-dot" aria-hidden="true" /> : item.dot ? <span className="rail-dot upd" aria-hidden="true" /> : null}
                </span>
                <span className="lbl grow">{item.label}</span>
                {item.badge ? <span className="lbl rail-count">{item.badge}</span> : null}
              </Link>
            </span>
          );
        })}
        <span className="rail-fill" />
        <button type="button" className="rail-item quiet hov" title="Выйти" onClick={() => void logout()}>
          <span className="rail-icon">
            <Icon name="logout" />
          </span>
          <span className="lbl">Выйти</span>
        </button>
        <button type="button" className="rail-item quiet hov" onClick={onFlipTheme}>
          <span className="rail-icon">
            <Icon name={theme === 'dark' ? 'sun' : 'moon'} />
          </span>
          <span className="lbl">{ThemeLabel(theme)}</span>
        </button>
        <BuildChip build={build} canOpen={can('settings')} variant="rail" />
        <button type="button" className="rail-item quiet hov" aria-pressed={pinned} onClick={togglePin}>
          <span className="rail-icon">
            <Icon name="pin" />
          </span>
          <span className="lbl">Закрепить меню</span>
        </button>
        <button
          type="button"
          className="rail-who"
          aria-expanded={whoOpen}
          aria-label="Профиль администратора"
          onClick={() => setWhoOpen((v) => !v)}
        >
          <span className="avatar" aria-hidden="true">
            {initial(me.email)}
          </span>
          <span className="lbl">
            <span className="who-name" title={me.email ?? undefined}>
              {me.email ?? 'Администратор'}
            </span>
            <span className="who-role">{ROLE_LABEL[me.admin.role]}</span>
          </span>
        </button>
      </nav>
      {whoOpen ? (
        <div className="profile-menu rail-pop" ref={whoRef}>
          <div className="profile-head">
            <b title={me.email ?? undefined}>{me.email ?? 'Администратор'}</b>
            <span className="muted" style={{ fontSize: 12 }}>
              {ROLE_LABEL[me.admin.role]}
            </span>
          </div>
          <Link to="/admin/security" className="hov" onClick={() => setWhoOpen(false)}>
            <Icon name="user" size={18} />
            Безопасность
          </Link>
          <Link to={REPORT_PATH} state={{ from: path }} className="hov" onClick={() => setWhoOpen(false)}>
            <Icon name="flag" size={18} />
            Сообщить об ошибке
          </Link>
          <button type="button" className="hov" onClick={() => void logout()}>
            <Icon name="logout" size={18} />
            Выйти
          </button>
        </div>
      ) : null}
    </div>
  );
}

function TopBar({ items, path, theme, onFlipTheme, build }: MenuProps) {
  const { me, logout, can } = useSession();
  const [moreOpen, setMoreOpen] = useState(false);
  const [profileOpen, setProfileOpen] = useState(false);
  const moreRef = useOutside(moreOpen, () => setMoreOpen(false));
  const profileRef = useOutside(profileOpen, () => setProfileOpen(false));
  const location = useLocation();
  useEffect(() => {
    setMoreOpen(false);
    setProfileOpen(false);
  }, [location.pathname]);
  const main = items.filter((item) => item.group === 'core');
  const more = items.filter((item) => item.group !== 'core');
  const moreCurrent = more.some((item) => isCurrent(item, path));
  let lastGroup: NavItem['group'] | null = null;
  return (
    <header className="topbar">
      <div className="topbar-in">
        <Link to="/admin" className="topbar-home">
          <BrandMark name={me.brand_name} logo={me.logo_url} className="mark" />
          <span>{me.brand_name.toUpperCase()}</span>
        </Link>
        <nav aria-label="Разделы" className="tabs">
          {main.map((item) => (
            <Link key={item.key} to={item.to} className="tab hov" aria-current={isCurrent(item, path) ? 'page' : undefined}>
              <span>{item.label}</span>
              {item.badge ? <span className="tab-count">{item.badge}</span> : null}
            </Link>
          ))}
          {more.length > 0 ? (
            <div className="more-wrap" ref={moreRef}>
              <button
                type="button"
                className={moreCurrent ? 'tab more current hov' : 'tab more hov'}
                aria-expanded={moreOpen}
                onClick={() => setMoreOpen((v) => !v)}
              >
                <span>Ещё</span>
                {more.some((item) => item.dot) ? <span className="nav-dot" aria-hidden="true" /> : null}
                <Icon name="chevron-down" size={15} stroke={2} />
              </button>
              {moreOpen ? (
                <div className="more-menu">
                  {more.map((item) => {
                    const sep = lastGroup !== null && item.group !== lastGroup;
                    lastGroup = item.group;
                    return (
                      <span key={item.key} style={{ display: 'contents' }}>
                        {sep ? <span className="more-sep" aria-hidden="true" /> : null}
                        <Link to={item.to} className="hov" aria-current={isCurrent(item, path) ? 'page' : undefined} aria-label={item.dot ? `${item.label}, ${UPDATE_LABEL}` : undefined}>
                          {item.label}
                          {item.dot ? <span className="nav-dot" aria-hidden="true" /> : null}
                        </Link>
                      </span>
                    );
                  })}
                </div>
              ) : null}
            </div>
          ) : null}
        </nav>
        <div className="topbar-end">
          <BuildChip build={build} canOpen={can('settings')} variant="top" />
          <button type="button" className="icon-btn hov" aria-label={ThemeLabel(theme)} title={ThemeLabel(theme)} onClick={onFlipTheme}>
            <Icon name={theme === 'dark' ? 'sun' : 'moon'} />
          </button>
          <button type="button" className="icon-btn hov" aria-label="Выйти" title="Выйти" onClick={() => void logout()}>
            <Icon name="logout" />
          </button>
          <div className="profile-wrap" ref={profileRef}>
            <button
              type="button"
              className="avatar"
              aria-label="Профиль администратора"
              aria-expanded={profileOpen}
              onClick={() => setProfileOpen((v) => !v)}
              style={{ cursor: 'pointer', fontWeight: 700 }}
            >
              {initial(me.email)}
            </button>
            {profileOpen ? (
              <div className="profile-menu">
                <div className="profile-head">
                  <b title={me.email ?? undefined}>{me.email ?? 'Администратор'}</b>
                  <span className="muted" style={{ fontSize: 12 }}>
                    {ROLE_LABEL[me.admin.role]}
                  </span>
                </div>
                <Link to="/admin/security" className="hov" onClick={() => setProfileOpen(false)}>
                  <Icon name="user" size={18} />
                  Безопасность
                </Link>
                <Link to={REPORT_PATH} state={{ from: path }} className="hov" onClick={() => setProfileOpen(false)}>
                  <Icon name="flag" size={18} />
                  Сообщить об ошибке
                </Link>
                <button type="button" className="hov" onClick={() => void logout()}>
                  <Icon name="logout" size={18} />
                  Выйти
                </button>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </header>
  );
}

const PHONE_TABS = ['home', 'clients', 'support', 'money'];

function PhoneNav({ items, path, theme, onFlipTheme, build }: MenuProps) {
  const { me, logout, can } = useSession();
  const [open, setOpen] = useState(false);
  const location = useLocation();
  useEffect(() => {
    setOpen(false);
  }, [location.pathname]);
  useEffect(() => {
    if (!open) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open]);
  const tabs = PHONE_TABS.map((key) => items.find((item) => item.key === key)).filter((item): item is NavItem => item !== undefined);
  const more = items.filter((item) => !PHONE_TABS.includes(item.key));
  const moreCurrent = more.some((item) => isCurrent(item, path));
  return (
    <>
      {open ? (
        <div className="psheet-back" role="presentation" onClick={() => setOpen(false)}>
          <div className="psheet" role="dialog" aria-modal="true" aria-label="Ещё" onClick={(event) => event.stopPropagation()}>
            <div className="psheet-list">
              {more.map((item) => (
                <Link key={item.key} to={item.to} className="psheet-row press" aria-current={isCurrent(item, path) ? 'page' : undefined} onClick={() => setOpen(false)}>
                  <Icon name={item.icon} size={21} />
                  <span className="psheet-name">
                    {item.label}
                    {item.dot ? <span className="nav-dot" aria-hidden="true" /> : null}
                  </span>
                  <Icon name="chevron-right" size={16} />
                </Link>
              ))}
              <Link to="/admin/security" className="psheet-row press" aria-current={path === '/admin/security' ? 'page' : undefined} onClick={() => setOpen(false)}>
                <Icon name="user" size={21} />
                <span className="psheet-name">Безопасность</span>
                <Icon name="chevron-right" size={16} />
              </Link>
              <Link to={REPORT_PATH} state={{ from: path }} className="psheet-row press" aria-current={path === REPORT_PATH ? 'page' : undefined} onClick={() => setOpen(false)}>
                <Icon name="flag" size={21} />
                <span className="psheet-name">Сообщить об ошибке</span>
                <Icon name="chevron-right" size={16} />
              </Link>
              <button type="button" className="psheet-row press" onClick={onFlipTheme}>
                <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={21} />
                <span className="psheet-name">{ThemeLabel(theme)}</span>
              </button>
              <button type="button" className="psheet-row press" onClick={() => void logout()}>
                <Icon name="logout" size={21} />
                <span className="psheet-name">Выйти</span>
              </button>
            </div>
            <div className="psheet-who">
              <span className="psheet-text">
                <b title={me.email ?? undefined}>{me.email ?? 'Администратор'}</b>
                <span className="muted">{ROLE_LABEL[me.admin.role]}</span>
              </span>
              <button type="button" className="psheet-out press" onClick={() => void logout()}>
                <Icon name="logout" size={18} />
                Выйти
              </button>
            </div>
            <VersionLine build={build} prefix="Версия" className="psheet-ver" canOpen={can('settings')} onOpen={() => setOpen(false)} />
          </div>
        </div>
      ) : null}
      <nav aria-label="Меню" className="pnav" style={{ gridTemplateColumns: `repeat(${tabs.length + (more.length ? 1 : 0)}, minmax(0, 1fr))` }}>
        {tabs.map((item) => (
          <Link
            key={item.key}
            to={item.to}
            className="pnav-tab press"
            aria-current={isCurrent(item, path) ? 'page' : undefined}
            aria-label={item.badge ? `${item.label}, ${item.badge} ${item.badgeLabel ?? ''}`.trim() : undefined}
          >
            <span className="pnav-icon">
              <Icon name={item.icon} size={22} />
              {item.badge ? (
                <span className="pnav-badge" aria-hidden="true">
                  {item.badge}
                </span>
              ) : null}
            </span>
            <span>{item.label}</span>
          </Link>
        ))}
        {more.length ? (
          <button type="button" className="pnav-tab press" aria-expanded={open} aria-current={moreCurrent && !open ? 'page' : undefined} onClick={() => setOpen((v) => !v)}>
            <span className="pnav-icon">
              <Icon name="more" size={22} />
              {more.some((item) => item.dot) ? <span className="nav-dot pnav-dot" aria-hidden="true" /> : null}
            </span>
            <span>Ещё</span>
          </button>
        ) : null}
      </nav>
    </>
  );
}

export function Shell({ theme, onFlipTheme, supportBadge, moneyBadge, build = null, children }: Props) {
  const { me, can } = useSession();
  const location = useLocation();
  const narrow = useMedia('(max-width: 760px)');
  const phone = usePhone();
  const layout = narrow ? 'top' : me.appearance.admin;
  const items = navItems(me, can, supportBadge, !!build?.update, moneyBadge);
  if (phone) {
    return (
      <div className="adm" data-layout="phone">
        <main className="page">{children}</main>
        <PhoneNav items={items} path={location.pathname} theme={theme} onFlipTheme={onFlipTheme} build={build} />
      </div>
    );
  }
  return (
    <div className="adm" data-layout={layout}>
      {layout === 'side' ? (
        <Rail items={items} path={location.pathname} theme={theme} onFlipTheme={onFlipTheme} build={build} />
      ) : (
        <TopBar items={items} path={location.pathname} theme={theme} onFlipTheme={onFlipTheme} build={build} />
      )}
      <main className="page">{children}</main>
    </div>
  );
}
