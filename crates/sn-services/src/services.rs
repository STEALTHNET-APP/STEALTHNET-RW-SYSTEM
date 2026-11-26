use crate::admin::AdminOps;
use crate::auth::Auth;
use crate::autorenew::AutoRenew;
use crate::bot_runtime::BotRuntime;
use crate::checkout::Checkout;
use crate::clients::ClientOps;
use crate::clock::Clock;
use crate::error::{ServiceError, ServiceResult};
use crate::events::EventDispatcher;
use crate::fulfillment::Fulfillment;
use crate::inbox::PaymentInbox;
use crate::infra::InfraOps;
use crate::limits::RateLimits;
use crate::remna_sync::RemnaSync;
use crate::server_clock::{ServerClock, ServerClockOps};
use crate::settings::{self, SettingsOps};
use crate::stars_review::StarsReview;
use crate::subscriptions::SubscriptionOps;
use crate::support::SupportOps;
use crate::tariffs::TariffAdmin;
use chrono::{DateTime, Utc};
use sn_domain::client::Locale;
use sn_domain::settings::CoreSettings;
use sn_modules::{BotSender, ModuleContext, ModuleRegistry};
use sn_payments::ProviderRegistry;
use sn_remnawave::{GuardedRemnawave, RemnaConfig, RemnawaveApi, RemnawaveClient};
use sn_store::{Db, SecretBox, Store};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemnaConnection {
    pub url: String,
    pub token: String,
    pub cookie: Option<String>,
    pub api_key: Option<String>,
    pub webhook_secret: Option<String>,
}

impl RemnaConnection {
    pub fn client_config(&self) -> RemnaConfig {
        let mut config = RemnaConfig::new(&self.url, &self.token);
        config.cookie = self.cookie.clone();
        config.api_key = self.api_key.clone();
        config
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFallbacks {
    pub public_url: Option<String>,
    pub remna: Option<RemnaConnection>,
    pub bot_token: Option<String>,
    pub telegram_api: Option<String>,
    pub ext_modules_loopback: bool,
    pub proxy: Option<sn_egress::EnvProxy>,
}

pub struct ServiceDeps {
    pub store: Store,
    pub payments: Arc<ProviderRegistry>,
    pub modules: Arc<ModuleRegistry>,
    pub secrets: Option<SecretBox>,
    pub clock: Arc<dyn Clock>,
    pub env: EnvFallbacks,
}

struct Runtime {
    settings: Arc<CoreSettings>,
    landing_style: Option<Arc<crate::own_css::StyleFile>>,
    miniapp_style: Option<Arc<crate::own_css::StyleFile>>,
    cabinet_style: Option<Arc<crate::own_css::StyleFile>>,
    remna: Option<Arc<dyn RemnawaveApi>>,
    remna_webhook_secret: Option<String>,
    bot_token: Option<String>,
    bot_webhook: Option<crate::bot_webhook::WebhookKeys>,
    updates: Option<Arc<crate::updates::UpdateConfig>>,
    backup_env: Option<Arc<crate::backups::BackupEnv>>,
    bot_content: Arc<crate::bot_content::BotContent>,
    bot_content_en: Arc<crate::bot_content::BotContent>,
    bot_sender: Option<Arc<dyn BotSender>>,
    oauth_endpoints: Arc<crate::oauth::OauthEndpoints>,
    captcha_endpoints: Arc<sn_modules::catalog::antibot::captcha::Endpoints>,
}

struct Inner {
    store: Store,
    payments: Arc<ProviderRegistry>,
    modules: Arc<ModuleRegistry>,
    secrets: Option<SecretBox>,
    clock: Arc<dyn Clock>,
    env: EnvFallbacks,
    instance: Uuid,
    runtime: RwLock<Runtime>,
    bot: BotRuntime,
    bot_updates: tokio::sync::Notify,
    flood: crate::bot_flood::FloodGate,
    sieve: crate::bot_flood::Sieve,
    pause: crate::pause::PauseGate,
    backups: crate::backups::Runtime,
    squad_countries: crate::squad_countries::SquadCountries,
    egress: sn_egress::Egress,
    captcha_http: sn_egress::Http,
    server_clock: Arc<ServerClock>,
}

#[derive(Clone)]
pub struct Services {
    inner: Arc<Inner>,
}

impl Services {
    pub fn new(deps: ServiceDeps) -> Self {
        let runtime = Runtime {
            settings: Arc::new(CoreSettings {
                public_url: deps.env.public_url.clone(),
                ..CoreSettings::default()
            }),
            landing_style: None,
            miniapp_style: None,
            cabinet_style: None,
            remna: None,
            remna_webhook_secret: deps
                .env
                .remna
                .as_ref()
                .and_then(|r| r.webhook_secret.clone()),
            bot_token: deps.env.bot_token.clone(),
            bot_webhook: None,
            updates: None,
            backup_env: None,
            bot_content: Arc::default(),
            bot_content_en: Arc::new(
                crate::bot_content::BotContent::default().in_locale(Locale::En),
            ),
            bot_sender: None,
            oauth_endpoints: Arc::default(),
            captcha_endpoints: Arc::default(),
        };
        let egress = sn_egress::Egress::new();
        let captcha_http = sn_modules::catalog::antibot::captcha::client().bound(&egress);
        let server_clock = Arc::new(ServerClock::new(deps.clock.clone()));
        Services {
            inner: Arc::new(Inner {
                store: deps.store,
                payments: deps.payments,
                modules: deps.modules,
                secrets: deps.secrets,
                clock: deps.clock,
                env: deps.env,
                instance: Uuid::new_v4(),
                runtime: RwLock::new(runtime),
                bot: BotRuntime::default(),
                bot_updates: tokio::sync::Notify::new(),
                flood: crate::bot_flood::FloodGate::default(),
                sieve: crate::bot_flood::Sieve::default(),
                pause: crate::pause::PauseGate::default(),
                backups: crate::backups::Runtime::default(),
                squad_countries: crate::squad_countries::SquadCountries::default(),
                egress,
                captcha_http,
                server_clock,
            }),
        }
    }

    fn read<T>(&self, f: impl FnOnce(&Runtime) -> T) -> T {
        match self.inner.runtime.read() {
            Ok(guard) => f(&guard),
            Err(poisoned) => f(&poisoned.into_inner()),
        }
    }

    fn write(&self, f: impl FnOnce(&mut Runtime)) {
        match self.inner.runtime.write() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn payments(&self) -> &Arc<ProviderRegistry> {
        &self.inner.payments
    }

    pub fn modules(&self) -> &Arc<ModuleRegistry> {
        &self.inner.modules
    }

    pub fn secrets(&self) -> Option<&SecretBox> {
        self.inner.secrets.as_ref()
    }

    pub fn env(&self) -> &EnvFallbacks {
        &self.inner.env
    }

    pub fn egress(&self) -> &sn_egress::Egress {
        &self.inner.egress
    }

    pub fn egress_ops(&self) -> crate::egress::EgressOps<'_> {
        crate::egress::EgressOps::new(self)
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.inner.clock.now()
    }

    pub fn server_clock(&self) -> &Arc<ServerClock> {
        &self.inner.server_clock
    }

    pub fn server_clock_ops(&self) -> ServerClockOps<'_> {
        ServerClockOps::new(self)
    }

    pub fn instance_id(&self) -> Uuid {
        self.inner.instance
    }

    pub async fn notify_settings_changed(&self) -> ServiceResult<()> {
        let mut conn = self.store().conn().await?;
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(settings::SETTINGS_CHANNEL)
            .bind(self.inner.instance.to_string())
            .execute(conn.conn())
            .await?;
        Ok(())
    }

    pub fn settings(&self) -> Arc<CoreSettings> {
        self.read(|r| r.settings.clone())
    }

    pub fn replace_settings(&self, settings: CoreSettings) {
        self.inner.modules.set_currency(settings.currency);
        let landing_style = crate::own_css::StyleFile::of(&settings.landing.css);
        let miniapp_style = crate::own_css::StyleFile::of(&settings.miniapp.css);
        let cabinet_style = crate::own_css::StyleFile::of(&settings.cabinet_css);
        self.write(|r| {
            r.settings = Arc::new(settings);
            r.landing_style = landing_style;
            r.miniapp_style = miniapp_style;
            r.cabinet_style = cabinet_style;
        });
    }

    pub fn landing_style(&self) -> Option<Arc<crate::own_css::StyleFile>> {
        self.read(|r| r.landing_style.clone())
    }

    pub fn miniapp_style(&self) -> Option<Arc<crate::own_css::StyleFile>> {
        self.read(|r| r.miniapp_style.clone())
    }

    pub fn cabinet_style(&self) -> Option<Arc<crate::own_css::StyleFile>> {
        self.read(|r| r.cabinet_style.clone())
    }

    pub fn remna(&self) -> ServiceResult<Arc<dyn RemnawaveApi>> {
        self.read(|r| r.remna.clone()).ok_or_else(|| {
            ServiceError::RemnaNotConfigured("укажите адрес панели и API-токен в настройках".into())
        })
    }

    pub fn set_remna(&self, api: Option<Arc<dyn RemnawaveApi>>) {
        self.write(|r| r.remna = api);
    }

    pub fn remna_webhook_secret(&self) -> Option<String> {
        self.read(|r| r.remna_webhook_secret.clone())
    }

    pub fn set_remna_webhook_secret(&self, secret: Option<String>) {
        self.write(|r| r.remna_webhook_secret = secret);
    }

    pub fn bot_token(&self) -> Option<String> {
        self.read(|r| r.bot_token.clone())
    }

    pub fn bot_webhook_keys(&self) -> Option<crate::bot_webhook::WebhookKeys> {
        self.read(|r| r.bot_webhook.clone())
    }

    pub fn bot_updates_signal(&self) -> &tokio::sync::Notify {
        &self.inner.bot_updates
    }

    pub fn bot_flood(&self) -> &crate::bot_flood::FloodGate {
        &self.inner.flood
    }

    pub fn bot_sieve(&self) -> &crate::bot_flood::Sieve {
        &self.inner.sieve
    }

    pub fn bot_webhook(&self) -> crate::bot_webhook::BotWebhook<'_> {
        crate::bot_webhook::BotWebhook::new(self)
    }

    pub fn set_bot_sender(&self, sender: Option<Arc<dyn BotSender>>) {
        self.write(|r| r.bot_sender = sender);
    }

    pub fn bot_sender(&self) -> Option<Arc<dyn BotSender>> {
        self.read(|r| r.bot_sender.clone())
    }

    pub fn bot_runtime(&self) -> &BotRuntime {
        &self.inner.bot
    }

    pub fn oauth_endpoints(&self) -> Arc<crate::oauth::OauthEndpoints> {
        self.read(|r| r.oauth_endpoints.clone())
    }

    pub fn set_oauth_endpoints(&self, endpoints: crate::oauth::OauthEndpoints) {
        self.write(|r| r.oauth_endpoints = Arc::new(endpoints));
    }

    pub fn captcha_endpoints(&self) -> Arc<sn_modules::catalog::antibot::captcha::Endpoints> {
        self.read(|r| r.captcha_endpoints.clone())
    }

    pub fn captcha_http(&self) -> sn_egress::Http {
        self.inner.captcha_http.clone()
    }

    pub fn set_captcha_endpoints(
        &self,
        endpoints: sn_modules::catalog::antibot::captcha::Endpoints,
    ) {
        self.write(|r| r.captcha_endpoints = Arc::new(endpoints));
    }

    pub async fn squad_countries(
        &self,
        squads: &[Uuid],
    ) -> std::collections::HashMap<Uuid, Vec<String>> {
        match self.remna() {
            Ok(remna) => {
                self.inner
                    .squad_countries
                    .of(&*remna, squads, std::time::Instant::now())
                    .await
            }
            Err(_) => std::collections::HashMap::new(),
        }
    }

    pub fn forget_squad_countries(&self) {
        self.inner.squad_countries.forget();
    }

    pub fn module_context(&self) -> ModuleContext {
        ModuleContext {
            store: self.inner.store.clone(),
            core: Arc::new(self.clone()),
            currency: self.settings().currency,
        }
    }

    pub async fn reload(&self) -> ServiceResult<()> {
        let mut conn = self.store().conn().await?;
        let mut core = settings::load_core(&mut conn).await?;
        if core.public_url.is_none() {
            core.public_url = self.inner.env.public_url.clone();
        }
        let remna = settings::remna_connection(&mut conn, self.secrets(), &self.inner.env).await?;
        let bot_token = settings::secret_or(
            &mut conn,
            self.secrets(),
            sn_domain::settings::secret_keys::BOT_TOKEN,
            self.inner.env.bot_token.clone(),
        )
        .await?;
        let snapshot = settings::registry_snapshot(
            &mut conn,
            self.secrets(),
            &self.inner.payments,
            &core,
            bot_token.as_deref(),
        )
        .await?;
        let checks = settings::provider_checks(&mut conn, &self.inner.payments).await?;
        let bot_content = crate::bot_content::load(&mut conn).await?;
        let bot_webhook = settings::read_secret(
            &mut conn,
            self.secrets(),
            sn_domain::settings::secret_keys::BOT_WEBHOOK,
        )
        .await?
        .and_then(|secret| crate::bot_webhook::WebhookKeys::from_secret(&secret));
        let plan =
            crate::egress::load_plan(&mut conn, self.secrets(), self.inner.env.proxy.as_ref())
                .await?;
        drop(conn);
        let telegram = self
            .inner
            .egress
            .proxy_for(sn_egress::Route::Telegram, None);
        self.inner.egress.set_plan(plan);
        let proxy_changed = telegram
            != self
                .inner
                .egress
                .proxy_for(sn_egress::Route::Telegram, None);
        self.inner.payments.apply(snapshot);
        self.inner.payments.set_checks(checks);
        self.inner.modules.set_currency(core.currency);
        self.inner.modules.load(self.store()).await?;
        let mut conn = self.store().conn().await?;
        let module_rows =
            settings::module_secret_rows(&mut conn, self.secrets(), &self.inner.modules).await?;
        drop(conn);
        self.inner.modules.apply_rows(&module_rows);
        let client: Option<Arc<dyn RemnawaveApi>> =
            if remna.url.is_empty() || remna.token.is_empty() {
                None
            } else {
                match RemnawaveClient::new(remna.client_config()) {
                    Ok(client) => Some(Arc::new(GuardedRemnawave::new(client))),
                    Err(err) => {
                        tracing::warn!(error = %err, "подключение к Remnawave не настроено");
                        None
                    }
                }
            };
        let previous = self.settings();
        let delivery_changed = previous.bot_updates != core.bot_updates
            || previous.public_url != core.public_url
            || self.bot_webhook_keys() != bot_webhook;
        let token_changed = self.bot_token() != bot_token;
        let landing_style = crate::own_css::StyleFile::of(&core.landing.css);
        let miniapp_style = crate::own_css::StyleFile::of(&core.miniapp.css);
        let cabinet_style = crate::own_css::StyleFile::of(&core.cabinet_css);
        self.write(|r| {
            r.settings = Arc::new(core);
            r.landing_style = landing_style;
            r.miniapp_style = miniapp_style;
            r.cabinet_style = cabinet_style;
            r.remna = client;
            r.remna_webhook_secret = remna.webhook_secret;
            r.bot_token = bot_token;
            r.bot_webhook = bot_webhook;
            r.bot_content_en = Arc::new(bot_content.in_locale(Locale::En));
            r.bot_content = Arc::new(bot_content);
        });
        if token_changed || delivery_changed || proxy_changed {
            self.inner.bot.wake();
        }
        Ok(())
    }

    pub fn auth(&self) -> Auth<'_> {
        Auth::new(self)
    }

    pub fn alerts(&self) -> crate::alerts::AlertsOps<'_> {
        crate::alerts::AlertsOps::new(self)
    }

    pub fn admins(&self) -> crate::admins::AdminsOps<'_> {
        crate::admins::AdminsOps::new(self)
    }

    pub fn api_keys(&self) -> crate::api_keys::ApiKeysOps<'_> {
        crate::api_keys::ApiKeysOps::new(self)
    }

    pub fn audit_journal(&self) -> crate::audit_journal::AuditJournalOps<'_> {
        crate::audit_journal::AuditJournalOps::new(self)
    }

    pub fn ext_modules(&self) -> crate::ext_modules::ExtModulesOps<'_> {
        crate::ext_modules::ExtModulesOps::new(self)
    }

    pub fn oauth(&self) -> crate::oauth::OauthOps<'_> {
        crate::oauth::OauthOps::new(self)
    }

    pub fn telegram_links(&self) -> crate::telegram_link::TelegramLinks<'_> {
        crate::telegram_link::TelegramLinks::new(self)
    }

    pub fn mail(&self) -> crate::mail::MailOps<'_> {
        crate::mail::MailOps::new(self)
    }

    pub fn letters(&self) -> crate::mail_letters::LettersOps<'_> {
        crate::mail_letters::LettersOps::new(self)
    }

    pub fn clients(&self) -> ClientOps<'_> {
        ClientOps::new(self)
    }

    pub fn checkout(&self) -> Checkout<'_> {
        Checkout::new(self)
    }

    pub fn inbox(&self) -> PaymentInbox<'_> {
        PaymentInbox::new(self)
    }

    pub fn grants(&self) -> crate::grants::Grants<'_> {
        crate::grants::Grants::new(self)
    }

    pub fn fulfillment(&self) -> Fulfillment<'_> {
        Fulfillment::new(self)
    }

    pub fn subscriptions(&self) -> SubscriptionOps<'_> {
        SubscriptionOps::new(self)
    }

    pub fn referral_links(&self) -> crate::referral_links::ReferralLinkOps<'_> {
        crate::referral_links::ReferralLinkOps::new(self)
    }

    pub fn remna_sync(&self) -> RemnaSync<'_> {
        RemnaSync::new(self)
    }

    pub fn remna_links(&self) -> crate::remna_link::RemnaLinks<'_> {
        crate::remna_link::RemnaLinks::new(self)
    }

    pub fn node_watch(&self) -> crate::node_watch::NodeWatch<'_> {
        crate::node_watch::NodeWatch::new(self)
    }

    pub fn autorenew(&self) -> AutoRenew<'_> {
        AutoRenew::new(self)
    }

    pub fn renewals(&self) -> crate::renewals::Renewals<'_> {
        crate::renewals::Renewals::new(self)
    }

    pub fn events(&self) -> EventDispatcher<'_> {
        EventDispatcher::new(self)
    }

    pub fn support(&self) -> SupportOps<'_> {
        SupportOps::new(self)
    }

    pub fn limits(&self) -> RateLimits<'_> {
        RateLimits::new(self)
    }

    pub fn admin(&self) -> AdminOps<'_> {
        AdminOps::new(self)
    }

    pub fn decisions(&self) -> crate::decisions::DecisionsOps<'_> {
        crate::decisions::DecisionsOps::new(self)
    }

    pub fn tariffs(&self) -> TariffAdmin<'_> {
        TariffAdmin::new(self)
    }

    pub fn promotions(&self) -> crate::promotions::Promotions<'_> {
        crate::promotions::Promotions::new(self)
    }

    pub fn update_config(&self) -> Option<Arc<crate::updates::UpdateConfig>> {
        self.read(|r| r.updates.clone())
    }

    pub fn set_update_config(&self, config: Option<crate::updates::UpdateConfig>) {
        self.write(|r| r.updates = config.map(Arc::new));
    }

    pub fn updates(&self) -> crate::updates::Updates<'_> {
        crate::updates::Updates::new(self)
    }

    pub fn pause_gate(&self) -> &crate::pause::PauseGate {
        &self.inner.pause
    }

    pub fn backups_runtime(&self) -> &crate::backups::Runtime {
        &self.inner.backups
    }

    pub fn backup_env(&self) -> Option<Arc<crate::backups::BackupEnv>> {
        self.read(|r| r.backup_env.clone())
    }

    pub fn set_backup_env(&self, env: Option<crate::backups::BackupEnv>) {
        self.write(|r| r.backup_env = env.map(Arc::new));
    }

    pub fn backups(&self) -> crate::backups::BackupOps<'_> {
        crate::backups::BackupOps::new(self)
    }

    pub fn infra(&self) -> InfraOps<'_> {
        InfraOps::new(self)
    }

    pub fn settings_ops(&self) -> SettingsOps<'_> {
        SettingsOps::new(self)
    }

    pub fn branding(&self) -> crate::branding::BrandingOps<'_> {
        crate::branding::BrandingOps::new(self)
    }

    pub fn landing(&self) -> crate::landing::LandingOps<'_> {
        crate::landing::LandingOps::new(self)
    }

    pub fn miniapp(&self) -> crate::miniapp::MiniAppOps<'_> {
        crate::miniapp::MiniAppOps::new(self)
    }

    pub fn cabinet(&self) -> crate::cabinet::CabinetOps<'_> {
        crate::cabinet::CabinetOps::new(self)
    }

    pub fn maintenance(&self) -> crate::maintenance::MaintenanceOps<'_> {
        crate::maintenance::MaintenanceOps::new(self)
    }

    pub fn connect_apps(&self) -> crate::connect_apps::ConnectAppsOps<'_> {
        crate::connect_apps::ConnectAppsOps::new(self)
    }

    pub fn app_links(&self) -> crate::app_links::AppLinks<'_> {
        crate::app_links::AppLinks::new(self)
    }

    pub fn bot_content(&self) -> Arc<crate::bot_content::BotContent> {
        self.read(|r| r.bot_content.clone())
    }

    pub fn bot_content_in(&self, locale: Locale) -> Arc<crate::bot_content::BotContent> {
        let locale = self.settings().languages.effective(locale);
        self.read(|r| match locale {
            Locale::Ru => r.bot_content.clone(),
            Locale::En => r.bot_content_en.clone(),
        })
    }

    pub fn locale_of(&self, locale: Locale) -> Locale {
        self.settings().languages.effective(locale)
    }

    pub fn documents(&self) -> crate::documents::DocumentOps<'_> {
        crate::documents::DocumentOps::new(self)
    }

    pub fn bot_content_ops(&self) -> crate::bot_content::BotContentOps<'_> {
        crate::bot_content::BotContentOps::new(self)
    }

    pub fn bot_buttons(&self) -> crate::bot_buttons::BotButtons<'_> {
        crate::bot_buttons::BotButtons::new(self)
    }

    pub fn bot_layout(&self) -> crate::bot_layout::BotLayoutOps<'_> {
        crate::bot_layout::BotLayoutOps::new(self)
    }

    pub fn stars_review(&self) -> StarsReview<'_> {
        StarsReview::new(self)
    }
}
