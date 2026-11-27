import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import type { Lang } from '../../lib/i18n';
import { Icon } from '../../lib/icons';
import { ACTION_KEYS, isSection, TAB_KEYS, type Banner, type Header, type HomeBlock, type Item, type ProfileBlock, type SectionKey, type Tab } from '../../mini/config';
import { actionText, EMPTY_TEXTS, FRIENDS_TEXTS, rowText, tabText } from '../../mini/labels';
import { Switch } from '../ui';
import { AddSelect, AudienceField, GlyphField, Grip, MoveButtons, PhraseField, PictureField, sectionOff, TargetField, useDragOrder, weight } from './fields';
import {
  audienceText,
  bannerProblem,
  itemIcon,
  itemName,
  linkText,
  moved,
  ownItemProblem,
  SECTION_TITLES,
  TAB_NOTES,
  tabFixed,
  type ImageRules,
  type Limits,
  type SectionInfo,
} from './model';

export type Kit = {
  lang: Lang;
  phone: boolean;
  limits: Limits;
  rules: ImageRules;
  sections: SectionInfo[];
  text: (ru: string) => string;
};

type BodyProps<B> = { block: B; change: (next: B) => void; kit: Kit };

function replace<T>(list: T[], index: number, next: T): T[] {
  return list.map((item, i) => (i === index ? next : item));
}

type RowProps = {
  name: string;
  note?: string | null;
  bad?: string | null;
  icon?: ReactNode;
  open: boolean;
  muted?: boolean;
  onOpen: () => void;
  grip?: ReactNode;
  end: ReactNode;
  drag?: ReturnType<ReturnType<typeof useDragOrder>['row']>;
  mark?: string;
  children?: ReactNode;
};

function Row({ name, note, bad, icon, open, muted, onOpen, grip, end, drag, mark = '', children }: RowProps) {
  return (
    <div className={`mce-item${open ? ' open' : ''}${muted ? ' muted' : ''}${mark}`} data-drag="" {...drag}>
      <div className="mce-item-head">
        {grip}
        {icon}
        <button type="button" className="mce-item-main" aria-expanded={open} onClick={onOpen}>
          <span className="mce-item-text">
            <span className="mce-item-name">{name}</span>
            {bad ? <span className="err">{bad}</span> : note ? <span className="cap">{note}</span> : null}
          </span>
          <Icon name="chevron-down" size={16} />
        </button>
        {end}
      </div>
      {open ? <div className="mce-item-body">{children}</div> : null}
    </div>
  );
}

function ItemsList({ items, onChange, kit, standard, labelMax, removable, what }: { items: Item[]; onChange: (next: Item[]) => void; kit: Kit; standard: (key: SectionKey) => string; labelMax: number; removable: boolean; what: string }) {
  const last = items[items.length - 1];
  const added = !!last && !last.std && !last.label?.ru;
  const [open, setOpen] = useState<number | null>(added ? items.length - 1 : null);
  const size = useRef(items.length);
  useEffect(() => {
    if (items.length > size.current && added) {
      setOpen(items.length - 1);
    }
    size.current = items.length;
  }, [items.length, added]);
  const move = (from: number, to: number) => {
    onChange(moved(items, from, to));
    setOpen((current) => (current === from ? to : null));
  };
  const order = useDragOrder(what, move);
  const ruText = (key: SectionKey) => standard(key);
  return (
    <div className="mce-items">
      {items.map((item, index) => {
        const name = itemName(item, ruText);
        const bad = item.std ? null : ownItemProblem(item, labelMax, kit.limits);
        const off = item.std ? sectionOff(kit.sections, item.std) : isSection(item.to) ? sectionOff(kit.sections, item.to) : null;
        const renamed = item.std && item.label?.ru?.trim() ? SECTION_TITLES[item.std] : null;
        const note = off ?? renamed ?? (item.std === 'addons' ? 'Виден, когда подписка работает' : item.std ? null : `${linkText(item.to)} · ${audienceText(item.for)}`);
        const hidden = item.on === false;
        const set = (next: Item) => onChange(replace(items, index, next));
        return (
          <Row
            key={item.std ?? `own-${index}`}
            name={name}
            note={note}
            bad={bad ? bad.charAt(0).toUpperCase() + bad.slice(1) : null}
            muted={hidden || !!off}
            icon={<Icon name={itemIcon(item)} size={18} />}
            grip={<Grip order={order} index={index} phone={kit.phone} name={name} />}
            drag={order.row(index)}
            mark={order.mark(index)}
            open={open === index}
            onOpen={() => setOpen(open === index ? null : index)}
            end={
              removable ? (
                <button type="button" className="mce-icon hov" aria-label={`Убрать: ${name}`} onClick={() => (onChange(items.filter((_, i) => i !== index)), setOpen(null))}>
                  <Icon name="close" size={16} />
                </button>
              ) : (
                <Switch on={!hidden} onToggle={() => set({ ...item, on: hidden ? undefined : false })} label={`Показывать «${name}»`} />
              )
            }
          >
            <PhraseField
              label="Название"
              value={item.label}
              lang={kit.lang}
              max={labelMax}
              standard={item.std ? kit.text(standard(item.std)) : null}
              onChange={(label) => set({ ...item, label })}
            />
            {item.std ? null : (
              <>
                <TargetField label="Куда ведёт" to={item.to} sections={kit.sections} max={kit.limits.url} onChange={(to) => set({ ...item, to: to ?? undefined })} />
                <GlyphField value={item.icon} shown={itemIcon(item)} onChange={(icon) => set({ ...item, icon })} />
                <AudienceField value={item.for} onChange={(next) => set({ ...item, for: next })} />
              </>
            )}
            <div className="mce-foot">
              <MoveButtons name={name} first={index === 0} last={index === items.length - 1} onUp={() => move(index, index - 1)} onDown={() => move(index, index + 1)} />
              {!item.std && !removable ? (
                <button
                  type="button"
                  className="mce-reset"
                  onClick={() => {
                    onChange(items.filter((_, i) => i !== index));
                    setOpen(null);
                  }}
                >
                  Удалить ссылку
                </button>
              ) : null}
            </div>
          </Row>
        );
      })}
    </div>
  );
}

function ownLink(): Item {
  return { label: {}, to: '' };
}

export function HeaderBody({ header, onChange }: { header: Header; onChange: (next: Header) => void }) {
  return (
    <>
      <label className="mce-toggle">
        <Switch on={header.balance} onToggle={() => onChange({ ...header, balance: !header.balance })} label="Показывать баланс в шапке" />
        <span>Показывать баланс</span>
      </label>
      <label className="mce-toggle">
        <Switch on={header.avatar} onToggle={() => onChange({ ...header, avatar: !header.avatar })} label="Показывать аватар клиента в шапке" />
        <span>Показывать аватар клиента, нажатие открывает профиль</span>
      </label>
      <span className="cap">
        Название меняется в{' '}
        <Link className="link" to="/admin/settings/general">
          «Общих»
        </Link>
        , логотип, цвет и тема — в{' '}
        <Link className="link" to="/admin/settings/look">
          «Оформлении»
        </Link>
      </span>
    </>
  );
}

function BannersBody({ block, change, kit }: BodyProps<Extract<HomeBlock, { kind: 'banners' }>>) {
  const items = block.items ?? [];
  const [open, setOpen] = useState<number | null>(items.length === 1 ? 0 : null);
  const commit = (next: Banner[]) => change({ ...block, items: next });
  const move = (from: number, to: number) => {
    commit(moved(items, from, to));
    setOpen((current) => (current === from ? to : null));
  };
  const order = useDragOrder('banner', move);
  return (
    <>
      {items.length ? (
        <div className="mce-items">
          {items.map((banner, index) => {
            const bad = bannerProblem(banner, kit.limits);
            const name = banner.title?.ru?.trim() || `Баннер ${index + 1}`;
            const set = (next: Banner) => commit(replace(items, index, next));
            const off = isSection(banner.to) ? sectionOff(kit.sections, banner.to) : null;
            const hidden = banner.on === false;
            const english = kit.lang === 'en';
            return (
              <Row
                key={index}
                name={name}
                note={off ?? `${banner.to ? linkText(banner.to) : 'Без ссылки'} · ${audienceText(banner.for)}`}
                bad={bad ? bad.charAt(0).toUpperCase() + bad.slice(1) : null}
                muted={hidden || !!off}
                grip={<Grip order={order} index={index} phone={kit.phone || items.length < 2} name={name} />}
                drag={order.row(index)}
                mark={order.mark(index)}
                open={open === index}
                onOpen={() => setOpen(open === index ? null : index)}
                end={<Switch on={!hidden} onToggle={() => set({ ...banner, on: hidden ? undefined : false })} label={`Показывать «${name}»`} />}
              >
                {english ? (
                  <PictureField
                    label="Картинка для английского"
                    image={banner.image_en}
                    rules={kit.rules}
                    what={name}
                    hint={banner.image ? 'Необязательно: без неё покажем картинку на русском' : `PNG, JPEG или WebP до ${weight(kit.rules.max_bytes)}`}
                    onChange={(image_en) => set({ ...banner, image_en: image_en ?? undefined })}
                  />
                ) : (
                  <PictureField
                    label="Картинка"
                    image={banner.image}
                    rules={kit.rules}
                    what={name}
                    hint={`PNG, JPEG или WebP до ${weight(kit.rules.max_bytes)}. Лучше горизонтальная, от 1000 точек в ширину`}
                    onChange={(image) => set({ ...banner, image: image ?? undefined })}
                  />
                )}
                <PhraseField label="Заголовок" value={banner.title} lang={kit.lang} max={kit.limits.banner_title} onChange={(title) => set({ ...banner, title })} />
                <PhraseField label="Текст" value={banner.text} lang={kit.lang} max={kit.limits.banner_text} multiline rows={2} onChange={(text) => set({ ...banner, text })} />
                <TargetField label="Куда ведёт" to={banner.to} none="Никуда" sections={kit.sections} max={kit.limits.url} onChange={(to) => set({ ...banner, to: to ?? undefined })} />
                <AudienceField value={banner.for} onChange={(next) => set({ ...banner, for: next })} />
                <div className="mce-foot">
                  {items.length > 1 ? (
                    <MoveButtons name={name} first={index === 0} last={index === items.length - 1} onUp={() => move(index, index - 1)} onDown={() => move(index, index + 1)} />
                  ) : null}
                  <button
                    type="button"
                    className="mce-reset"
                    onClick={() => {
                      commit(items.filter((_, i) => i !== index));
                      setOpen(null);
                    }}
                  >
                    Удалить баннер
                  </button>
                </div>
              </Row>
            );
          })}
        </div>
      ) : (
        <span className="cap">Баннер — картинка или карточка с текстом: акция, новость, ссылка на канал. Если баннеров несколько, их листают вбок</span>
      )}
      {items.length < kit.limits.banners ? (
        <button
          type="button"
          className="mce-add"
          onClick={() => {
            commit([...items, {}]);
            setOpen(items.length);
          }}
        >
          <Icon name="plus" size={16} />
          Добавить баннер
        </button>
      ) : (
        <span className="cap">Больше {kit.limits.banners} баннеров не поместится</span>
      )}
    </>
  );
}

function GreetingBody({ block, change, kit }: BodyProps<Extract<HomeBlock, { kind: 'greeting' }>>) {
  return (
    <>
      <PhraseField
        label="Заголовок"
        value={block.title}
        lang={kit.lang}
        max={kit.limits.greeting_title}
        placeholder={kit.lang === 'en' ? 'Hi, {name}!' : 'Привет, {name}!'}
        hint="{name} — имя клиента из Telegram. Если имени нет, текст останется без него"
        onChange={(title) => change({ ...block, title })}
      />
      <PhraseField label="Текст" value={block.text} lang={kit.lang} max={kit.limits.greeting_text} multiline onChange={(text) => change({ ...block, text })} />
      <AudienceField value={block.for} onChange={(next) => change({ ...block, for: next })} />
    </>
  );
}

function SubscriptionBody({ block, change, kit }: BodyProps<Extract<HomeBlock, { kind: 'subscription' }>>) {
  const look = block.look ?? 'ring';
  const empty = block.empty ?? {};
  const set = (patch: typeof empty) => change({ ...block, empty: { ...empty, ...patch } });
  return (
    <>
      <div className="mce-field">
        <span className="mce-label">Вид</span>
        <div role="group" aria-label="Вид подписки" className="segs mce-segs">
          <button type="button" aria-pressed={look === 'ring'} onClick={() => change({ ...block, look: undefined })}>
            Круг
          </button>
          <button type="button" aria-pressed={look === 'compact'} onClick={() => change({ ...block, look: 'compact' })}>
            Компактно
          </button>
        </div>
        <span className="cap">Дни, трафик и кнопка «Подключить» или «Продлить» есть в обоих видах</span>
      </div>
      <span className="mce-group">Когда подписки нет</span>
      <PhraseField label="Заголовок" value={empty.title} lang={kit.lang} max={kit.limits.empty_title} standard={kit.text(EMPTY_TEXTS.title)} onChange={(title) => set({ title })} />
      <PhraseField label="Строка под ним" value={empty.line} lang={kit.lang} max={kit.limits.empty_line} standard={kit.text(EMPTY_TEXTS.line)} hint="Пустое поле — строки не будет" onChange={(line) => set({ line })} />
      <PhraseField label="Кнопка" value={empty.button} lang={kit.lang} max={kit.limits.button} standard={kit.text(EMPTY_TEXTS.button)} hint="Всегда ведёт в «Тарифы»" onChange={(button) => set({ button })} />
    </>
  );
}

function ActionsBody({ block, change, kit }: BodyProps<Extract<HomeBlock, { kind: 'actions' }>>) {
  const items = block.items ?? [];
  const left = ACTION_KEYS.filter((key) => !items.some((item) => item.std === key)).map((key) => ({ key: key as string, name: SECTION_TITLES[key] }));
  return (
    <>
      {items.length ? (
        <ItemsList items={items} kit={kit} standard={actionText} labelMax={kit.limits.button} removable what="action" onChange={(next) => change({ ...block, items: next })} />
      ) : (
        <span className="cap">Кнопок нет</span>
      )}
      {items.length < kit.limits.actions ? (
        <AddSelect
          label="Добавить кнопку"
          options={[...left, { key: 'own', name: 'Своя ссылка' }]}
          onPick={(key) => change({ ...block, items: [...items, isSection(key) ? { std: key } : ownLink()] })}
        />
      ) : null}
      <span className="cap">До {kit.limits.actions} кнопок. «Продлить» и «Устройства» видны, когда подписка есть; у закончившейся вместо «Продлить» — «Подключить»</span>
    </>
  );
}

function SectionsBody({ block, change, kit }: BodyProps<Extract<HomeBlock, { kind: 'sections' }>>) {
  const items = block.items ?? [];
  const own = items.filter((item) => !item.std).length;
  return (
    <>
      <ItemsList items={items} kit={kit} standard={rowText} labelMax={kit.limits.label} removable={false} what="section" onChange={(next) => change({ ...block, items: next })} />
      {own < kit.limits.links ? (
        <button type="button" className="mce-add" onClick={() => change({ ...block, items: [...items, ownLink()] })}>
          <Icon name="plus" size={16} />
          Добавить свою ссылку
        </button>
      ) : null}
      <span className="cap">Скрытый раздел открывается по прямой ссылке и из бота. Поддержка и история оплат всегда есть в профиле</span>
    </>
  );
}

export function HomeBody({ block, change, kit }: BodyProps<HomeBlock>) {
  switch (block.kind) {
    case 'banners':
      return <BannersBody block={block} change={change} kit={kit} />;
    case 'greeting':
      return <GreetingBody block={block} change={change} kit={kit} />;
    case 'subscription':
      return <SubscriptionBody block={block} change={change} kit={kit} />;
    case 'actions':
      return <ActionsBody block={block} change={change} kit={kit} />;
    case 'sections':
      return <SectionsBody block={block} change={change} kit={kit} />;
  }
}

function FriendsBody({ block, change, kit, percent }: BodyProps<Extract<ProfileBlock, { kind: 'friends' }>> & { percent: number }) {
  const place = block.place ?? 'card';
  const off = sectionOff(kit.sections, 'friends');
  return (
    <>
      {off ? <span className="warn-line">{off}: клиенты блок не видят</span> : null}
      <div className="mce-field">
        <span className="mce-label">Где показывать</span>
        <div role="group" aria-label="Где показывать друзей" className="segs mce-segs">
          <button type="button" aria-pressed={place === 'card'} onClick={() => change({ ...block, place: undefined })}>
            В профиле
          </button>
          <button type="button" aria-pressed={place === 'screen'} onClick={() => change({ ...block, place: 'screen' })}>
            Отдельным экраном
          </button>
        </div>
        <span className="cap">{place === 'screen' ? 'В профиле останется строка, которая ведёт на экран' : 'Карточка с текстом и ссылками прямо в профиле'}</span>
      </div>
      <PhraseField label="Заголовок" value={block.title} lang={kit.lang} max={kit.limits.friends_title} standard={kit.text(FRIENDS_TEXTS.title)} onChange={(title) => change({ ...block, title })} />
      <PhraseField
        label="Текст"
        value={block.text}
        lang={kit.lang}
        max={kit.limits.friends_text}
        standard={kit.text(FRIENDS_TEXTS.text)}
        multiline
        hint={`Текст над строками с процентами. {percent} — процент первого уровня из модуля «Рефералы», сейчас ${percent}. Пустое поле — без текста`}
        onChange={(text) => change({ ...block, text })}
      />
    </>
  );
}

function HelpBody({ block, change, kit }: BodyProps<Extract<ProfileBlock, { kind: 'help' }>>) {
  const gifts = block.gifts === true;
  const off = sectionOff(kit.sections, 'gifts');
  return (
    <>
      <label className="mce-toggle">
        <Switch on={gifts} onToggle={() => change({ ...block, gifts: gifts ? undefined : true })} label="Показывать «Подарить подписку» в профиле" />
        <span>Показывать «Подарить подписку»</span>
      </label>
      {gifts && off ? <span className="warn-line">{off}: клиенты строку не видят</span> : null}
      <span className="cap">
        История оплат и поддержка есть у клиента всегда. Куда приходят обращения, решается в{' '}
        <Link className="link" to="/admin/settings/general">
          «Общих»
        </Link>
      </span>
    </>
  );
}

function LinksBody({ block, change, kit }: BodyProps<Extract<ProfileBlock, { kind: 'links' }>>) {
  const items = block.items ?? [];
  return (
    <>
      {items.length ? (
        <ItemsList items={items} kit={kit} standard={rowText} labelMax={kit.limits.label} removable={false} what="link" onChange={(next) => change({ ...block, items: next })} />
      ) : (
        <span className="cap">Канал, сайт, страница статуса — всё, что должно быть под рукой у клиента</span>
      )}
      {items.length < kit.limits.links ? (
        <button type="button" className="mce-add" onClick={() => change({ ...block, items: [...items, ownLink()] })}>
          <Icon name="plus" size={16} />
          Добавить ссылку
        </button>
      ) : null}
    </>
  );
}

export function ProfileBody({ block, change, kit, percent }: BodyProps<ProfileBlock> & { percent: number }) {
  switch (block.kind) {
    case 'person':
      return <span className="cap">Фото, имя и @username клиента из Telegram. Если фото нет или клиент его скрыл, виден значок</span>;
    case 'balance':
      return <span className="cap">Сумма и кнопка «Пополнить». Клиент с деньгами на балансе видит карточку, даже если она скрыта</span>;
    case 'friends':
      return <FriendsBody block={block} change={change} kit={kit} percent={percent} />;
    case 'links':
      return <LinksBody block={block} change={change} kit={kit} />;
    case 'help':
      return <HelpBody block={block} change={change} kit={kit} />;
    case 'documents':
      return (
        <span className="cap">
          Список берётся из раздела{' '}
          <Link className="link" to="/admin/settings/documents">
            «Документы»
          </Link>
          . Если документов нет, блока тоже нет
        </span>
      );
    case 'account':
      return (
        <span className="cap">
          Почта, пароль для сайта, вход по коду и язык. Состав зависит от{' '}
          <Link className="link" to="/admin/settings/client">
            «Кабинета и мини-приложения»
          </Link>
        </span>
      );
  }
}

export function TabsEditor({ tabs, onChange, kit }: { tabs: Tab[]; onChange: (next: Tab[]) => void; kit: Kit }) {
  const move = (from: number, to: number) => onChange(moved(tabs, from, to));
  const order = useDragOrder('tab', move);
  const left = TAB_KEYS.filter((key) => !tabs.some((tab) => tab.std === key)).map((key) => ({ key: key as string, name: SECTION_TITLES[key] }));
  return (
    <>
      <div className="card mce-list">
        {tabs.map((tab, index) => {
          const fixed = tabFixed(tab.std);
          const off = sectionOff(kit.sections, tab.std);
          const standard = kit.text(tabText(tab.std));
          return (
            <div key={tab.std} className={`mce-tab${order.mark(index)}`} data-drag="" {...order.row(index)}>
              <Grip order={order} index={index} phone={kit.phone} name={SECTION_TITLES[tab.std]} />
              <TabFields tab={tab} standard={standard} off={off} kit={kit} onChange={(label) => onChange(replace(tabs, index, { ...tab, label }))} />
              <span className="mce-tab-tools">
                <button type="button" className="mce-icon hov" aria-label={`${SECTION_TITLES[tab.std]}: выше`} disabled={index === 0} onClick={() => move(index, index - 1)}>
                  <Icon name="arrow-up" size={16} />
                </button>
                <button type="button" className="mce-icon hov" aria-label={`${SECTION_TITLES[tab.std]}: ниже`} disabled={index === tabs.length - 1} onClick={() => move(index, index + 1)}>
                  <Icon name="arrow-down" size={16} />
                </button>
                {fixed ? (
                  <span className="mce-icon" title={`Убрать нельзя: ${TAB_NOTES[tab.std] ?? ''}`} role="img" aria-label={`Вкладку «${SECTION_TITLES[tab.std]}» убрать нельзя`}>
                    <Icon name="lock" size={16} />
                  </span>
                ) : (
                  <button
                    type="button"
                    className="mce-icon hov"
                    aria-label={`Убрать вкладку «${SECTION_TITLES[tab.std]}»`}
                    disabled={tabs.length <= kit.limits.tabs_min}
                    onClick={() => onChange(tabs.filter((_, i) => i !== index))}
                  >
                    <Icon name="close" size={16} />
                  </button>
                )}
              </span>
            </div>
          );
        })}
      </div>
      {tabs.length < kit.limits.tabs_max ? <AddSelect label="Добавить вкладку" options={left} onPick={(key) => isSection(key) && onChange([...tabs, { std: key }])} /> : null}
      <span className="cap">
        От {kit.limits.tabs_min} до {kit.limits.tabs_max} вкладок, сейчас {tabs.length}. «Главная», «Тарифы» и «Профиль» остаются всегда: без них клиент не увидит подписку, не оплатит её и не найдёт поддержку
      </span>
    </>
  );
}

function TabFields({ tab, standard, off, kit, onChange }: { tab: Tab; standard: string; off: string | null; kit: Kit; onChange: (label: Tab['label']) => void }) {
  const note = off ? `${off}: вкладки у клиента не будет` : undefined;
  return (
    <div className={off ? 'mce-tab-main muted' : 'mce-tab-main'}>
      <Icon name={itemIcon({ std: tab.std })} size={20} />
      <PhraseField label={SECTION_TITLES[tab.std]} value={tab.label} lang={kit.lang} max={kit.limits.tab} standard={standard} hint={note} onChange={onChange} />
    </div>
  );
}
