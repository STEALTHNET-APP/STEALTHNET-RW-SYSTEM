import { BrandMark } from '../lib/brand';
import { t } from '../lib/i18n';
import { maintenanceText, untilText } from '../lib/maintenance';
import type { Maintenance, Site } from '../lib/types';

export function CabinetMaintenance({ site, mode }: { site: Site; mode: Maintenance }) {
  const until = untilText(mode.until);
  return (
    <div className="cb-solo">
      <div className="cb-solo-box cb-maint" role="status">
        <span className="cb-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="cb-mark" />
          <span>{site.brand_name.toUpperCase()}</span>
        </span>
        <h1>{t('Технические работы')}</h1>
        <p className="cb-maint-text">{maintenanceText(mode)}</p>
        {until ? <p>{until}</p> : null}
        {site.support_url ? (
          <a className="cb-second" href={site.support_url} target="_blank" rel="noopener noreferrer">
            {t('Написать в поддержку')}
          </a>
        ) : null}
      </div>
    </div>
  );
}
