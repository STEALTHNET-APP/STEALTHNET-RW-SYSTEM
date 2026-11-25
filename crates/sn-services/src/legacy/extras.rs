use super::mail::{self, MailPlan};
use super::model::{
    InvoicePurpose, LAdmin, LAutoRule, LGift, LInvoice, LWithdrawal, Legacy, Source,
    WithdrawalState,
};
use super::multi::{self, MultiPlan};
use super::runs::IssueRow;
use crate::Services;
use crate::bot_content::ContentUpdate;
use crate::documents::{ListItem, ListUpdate};
use crate::error::{ServiceError, ServiceResult};
use crate::settings::{CoreUpdate, ModuleConfigUpdate, ProviderUpdate};
use serde_json::{Map, Value};
use sn_domain::access::{AdminRole, Permission};
use sn_domain::bot_content::{ButtonOverride, ButtonStyle, clean_button, clean_text};
use sn_domain::client::{Languages, Locale};
use sn_domain::ids::{AdminId, ClientId, OrderId, PeriodId, SquadId, SubscriptionId, TariffId};
use sn_domain::module::ModuleId;
use sn_domain::money::Money;
use sn_domain::order::PurchaseIntent;
use sn_domain::settings::{SupportMode, TrialSettings};
use sn_domain::traffic::{TrafficPolicy, TrafficReset};
use sn_modules::catalog::broadcast::{
    BotSection, ButtonSpec, Channel, RuleDraft, Trigger, Unit, create_rule,
};
use sn_modules::catalog::gifts;
use sn_store::Db;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Done {
    pub settings: u64,
    pub providers: Vec<String>,
    pub mail: Option<String>,
    pub multi: bool,
    pub admins: u64,
    pub withdrawals: u64,
    pub gifts: u64,
    pub rules: u64,
    pub invoices: u64,
}

pub struct Extras<'a> {
    pub services: &'a Services,
    pub legacy: &'a Legacy,
    pub admin: Option<AdminId>,
    pub run_id: i64,
    pub clients: &'a HashMap<String, i64>,
    pub tariffs: &'a HashMap<String, i64>,
    pub tariff_names: &'a HashMap<i64, String>,
    pub periods_by_days: &'a HashMap<(i64, u32), i64>,
    pub periods: &'a HashMap<String, i64>,
    pub subs: &'a HashMap<String, i64>,
    pub issues: Vec<IssueRow>,
    pub done: Done,
}

fn value<'b>(old: &'b BTreeMap<String, String>, key: &str) -> Option<&'b str> {
    old.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
}

fn truthy(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

pub fn support_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(name) = raw.strip_prefix('@') {
        return (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .then(|| format!("https://t.me/{name}"));
    }
    if raw.starts_with("https://") || raw.starts_with("tg://") {
        return Some(raw.to_string());
    }
    if let Some(rest) = raw.strip_prefix("http://") {
        return Some(format!("https://{rest}"));
    }
    if raw.starts_with("t.me/") {
        return Some(format!("https://{raw}"));
    }
    (!raw.is_empty() && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .then(|| format!("https://t.me/{raw}"))
}

fn json(raw: Option<&str>) -> Option<Value> {
    serde_json::from_str(raw?).ok()
}

fn emoji_of(emojis: &Option<Value>, key: &str) -> (Option<String>, Option<String>) {
    match emojis.as_ref().and_then(|e| e.get(key)) {
        Some(Value::String(symbol)) => (Some(symbol.trim().to_string()), None),
        Some(Value::Object(entry)) => (
            entry
                .get("unicode")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            entry
                .get("tgEmojiId")
                .map(|v| match v {
                    Value::String(s) => s.trim().to_string(),
                    other => other.to_string(),
                })
                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())),
        ),
        _ => (None, None),
    }
}

pub fn old_text(raw: &str, emojis: &Option<Value>) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find("{{") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        let Some(end) = tail.find("}}") else {
            out.push_str(&rest[at..]);
            rest = "";
            break;
        };
        let name = tail[..end].trim();
        let (symbol, _) = emoji_of(emojis, name);
        if let Some(symbol) = symbol {
            out.push_str(&symbol);
        }
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    let mut bold = String::new();
    for (i, part) in out.split("**").enumerate() {
        if i % 2 == 1 {
            bold.push_str("<b>");
            bold.push_str(part);
            bold.push_str("</b>");
        } else {
            bold.push_str(part);
        }
    }
    if out.matches("**").count() % 2 == 1 {
        return out.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    bold.trim().to_string()
}

fn button_keys(id: &str) -> &'static [&'static str] {
    match id {
        "tariffs" => &["menu.btn.buy", "menu.btn.extend"],
        "topup" => &["menu.btn.balance"],
        "referral" => &["menu.btn.referral"],
        "trial" => &["menu.btn.trial"],
        "vpn" | "my_subs" => &["menu.btn.subscription"],
        "cabinet" => &["menu.btn.app"],
        "support" | "tickets" => &["menu.btn.support"],
        "promocode" => &["menu.btn.promo"],
        "extra_options" => &["menu.btn.options"],
        "gifts" | "gift" => &["menu.btn.gifts"],
        "contests" => &["menu.btn.contests"],
        "language" | "lang" => &["menu.btn.language"],
        _ => &[],
    }
}

fn style_of(raw: Option<&str>) -> Option<ButtonStyle> {
    match raw.map(str::trim) {
        Some("primary") => Some(ButtonStyle::Primary),
        Some("success") => Some(ButtonStyle::Success),
        Some("danger") => Some(ButtonStyle::Danger),
        _ => None,
    }
}

fn permissions_of(admin: &LAdmin) -> (AdminRole, Vec<Permission>) {
    if admin.full_access {
        return (AdminRole::Admin, Vec::new());
    }
    let mut out = BTreeSet::new();
    for raw in &admin.sections {
        let section = raw.trim().to_ascii_lowercase();
        let name = section.strip_prefix("action:").unwrap_or(&section);
        let grants: &[Permission] = match name {
            "clients" | "antibot" | "bulk_block" | "bulk_reset_trial" | "manage_services"
            | "delete_device" => &[Permission::ClientsRead, Permission::ClientsWrite],
            n if n.starts_with("change_") => &[Permission::ClientsRead, Permission::ClientsWrite],
            "dashboard" => &[Permission::ClientsRead],
            "tickets" => &[Permission::Support],
            "sales-report"
            | "analytics"
            | "traffic-abuse"
            | "geo-map"
            | "withdrawals"
            | "view_balance_sales"
            | "bulk_credit_balance"
            | "bulk_debit_balance" => &[Permission::Money],
            "refund_payment" | "mark_payment_failed" | "delete_payment" => &[Permission::Refunds],
            "tariffs"
            | "trials"
            | "auto-renew"
            | "secondary-subscriptions"
            | "tariffs_csv_import" => &[Permission::Tariffs],
            n if n.starts_with("promo") => &[Permission::Marketing],
            "marketing" | "referral-network" | "referrals" | "broadcast" | "auto-broadcast"
            | "contests" | "tour-constructor" | "video-instructions" => &[Permission::Marketing],
            "marketplace" => &[Permission::Modules],
            "remna-nodes" | "proxy" | "singbox" | "backup" | "diagnostics" | "webhook-inbox"
            | "trigger_cron" | "replay_webhook" => &[Permission::Infrastructure],
            "settings" | "languages" | "api-keys" | "bots" | "audit" => &[Permission::Settings],
            _ => &[],
        };
        out.extend(grants.iter().copied());
    }
    (AdminRole::ReadOnly, out.into_iter().collect())
}

fn trigger_of(rule: &LAutoRule) -> Option<(Trigger, u16)> {
    let days = u16::try_from(rule.days.clamp(0, 90)).unwrap_or(0);
    match rule.trigger.as_str() {
        "subscription_ending_soon" | "expiring" => Some((Trigger::Expiring, days.max(1))),
        "subscription_expired" | "expired" | "inactivity" => Some((Trigger::Expired, days)),
        "trial_used_never_paid" | "trial_ended" | "trial_expired" => {
            Some((Trigger::TrialEnded, days))
        }
        "no_payment" | "after_registration" | "no_purchase" if days >= 1 => {
            Some((Trigger::NoPurchase, days))
        }
        "no_traffic" | "not_connected" => Some((Trigger::NotConnected, days.max(1))),
        _ => None,
    }
}

impl<'a> Extras<'a> {
    fn issue(
        &mut self,
        code: &str,
        entity: &str,
        old_id: Option<&str>,
        who: Option<String>,
        details: impl Into<String>,
    ) {
        self.issues.push(IssueRow {
            stage: "apply",
            code: code.to_string(),
            entity: entity.to_string(),
            old_id: old_id.map(str::to_string),
            who,
            details: details.into(),
        });
    }

    fn entity(&self, kind: &str) -> String {
        self.legacy.source.entity(kind)
    }

    async fn actor(&self) -> ServiceResult<Option<AdminId>> {
        if let Some(admin) = self.admin {
            return Ok(Some(admin));
        }
        let mut conn = self.services.store().conn().await?;
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM admins WHERE role = 'owner' AND disabled_at IS NULL ORDER BY id LIMIT 1",
        )
        .fetch_optional(conn.conn())
        .await?;
        Ok(id.map(AdminId))
    }

    async fn known(&self, kind: &str) -> ServiceResult<HashMap<String, i64>> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT old_id, new_id FROM legacy_ids WHERE entity = $1")
                .bind(self.entity(kind))
                .fetch_all(conn.conn())
                .await?;
        Ok(rows.into_iter().collect())
    }

    async fn remember(
        &self,
        db: &mut impl Db,
        kind: &str,
        old_id: &str,
        new_id: i64,
    ) -> ServiceResult<()> {
        sqlx::query(
            "INSERT INTO legacy_ids (entity, old_id, new_id, data, run_id, updated_at) VALUES ($1, $2, $3, '{}', $4, now()) \
             ON CONFLICT (entity, old_id) DO UPDATE SET new_id = EXCLUDED.new_id, run_id = EXCLUDED.run_id, updated_at = now()",
        )
        .bind(self.entity(kind))
        .bind(old_id)
        .bind(new_id)
        .bind(self.run_id)
        .execute(db.conn())
        .await?;
        Ok(())
    }

    async fn present(&self) -> ServiceResult<HashSet<String>> {
        let mut conn = self.services.store().conn().await?;
        let keys: Vec<String> = sqlx::query_scalar("SELECT key FROM settings")
            .fetch_all(conn.conn())
            .await?;
        Ok(keys.into_iter().collect())
    }

    async fn module_fields(&self, module: &str) -> ServiceResult<HashSet<String>> {
        let mut conn = self.services.store().conn().await?;
        let config: Option<Value> =
            sqlx::query_scalar("SELECT config FROM module_states WHERE module = $1")
                .bind(module)
                .fetch_optional(conn.conn())
                .await?;
        Ok(config
            .as_ref()
            .and_then(Value::as_object)
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn outcome(&mut self, what: &str, result: ServiceResult<()>) {
        match result {
            Ok(()) => self.done.settings += 1,
            Err(err) => self.issue(
                "setting_skipped",
                "setting",
                None,
                Some(what.to_string()),
                format!("Не перенесено: {err}. Задайте вручную"),
            ),
        }
    }

    pub async fn run(&mut self) -> ServiceResult<()> {
        if self.legacy.source == Source::Stealthnet {
            let applied = self.known("settings").await?;
            if applied.is_empty() {
                match self.actor().await? {
                    Some(actor) => {
                        self.settings(actor).await;
                        self.providers(actor).await;
                        let mut conn = self.services.store().conn().await?;
                        self.remember(&mut conn, "settings", "applied", self.run_id)
                            .await?;
                    }
                    None => self.issue(
                        "settings_skipped",
                        "setting",
                        None,
                        None,
                        "В установке нет владельца — настройки бота и платёжек не перенесены. Создайте владельца и перенесите снова",
                    ),
                }
            } else if !self.legacy.settings.is_empty() {
                self.issue(
                    "settings_kept",
                    "setting",
                    None,
                    None,
                    "Настройки бота и платёжек уже переносились — повторный перенос их не трогает, чтобы не затереть ваши правки",
                );
            }
            self.mail().await?;
            self.multi().await?;
            self.admins().await?;
        }
        self.withdrawals().await?;
        self.gifts().await?;
        self.rules().await?;
        self.invoices().await?;
        Ok(())
    }

    async fn options(&self) -> ServiceResult<super::runs::StartOptions> {
        let mut conn = self.services.store().conn().await?;
        let options: Option<Value> =
            sqlx::query_scalar("SELECT options FROM import_runs WHERE id = $1")
                .bind(self.run_id)
                .fetch_optional(conn.conn())
                .await?;
        Ok(options
            .and_then(|o| serde_json::from_value::<super::runs::StartOptions>(o).ok())
            .unwrap_or_default())
    }

    async fn replace_mail(&self) -> ServiceResult<bool> {
        Ok(self.options().await?.replace_mail)
    }

    async fn multi(&mut self) -> ServiceResult<()> {
        let old = multi::old_multi(self.legacy);
        let entity = self.entity("multi");
        let plan = multi::plan(self.services, &entity, self.legacy).await?;
        let enable = match &plan {
            Some(MultiPlan::Enable { .. }) => true,
            Some(MultiPlan::Differs { .. }) => self.options().await?.replace_multi,
            Some(MultiPlan::Same) | None => false,
        };
        if enable {
            match self.actor().await? {
                Some(actor) => {
                    let value = sn_domain::settings::MultiSubscriptions { enabled: true };
                    match self
                        .services
                        .settings_ops()
                        .update_multi_subscriptions(&value, actor)
                        .await
                    {
                        Ok(_) => {
                            self.done.settings += 1;
                            self.done.multi = true;
                            let mut conn = self.services.store().conn().await?;
                            self.remember(&mut conn, "multi", "applied", self.run_id)
                                .await?;
                        }
                        Err(err) => self.issue(
                            "multi_skipped",
                            "setting",
                            None,
                            Some("Подписки".into()),
                            format!("Режим нескольких подписок не включён: {err}. Включите его в «Тарифах»"),
                        ),
                    }
                }
                None => self.issue(
                    "multi_skipped",
                    "setting",
                    None,
                    Some("Подписки".into()),
                    "В установке нет владельца — режим нескольких подписок не включён. Включите его в «Тарифах»",
                ),
            }
        } else if let Some(MultiPlan::Differs {
            setting, clients, ..
        }) = plan
        {
            self.issue(
                "multi_kept",
                "setting",
                None,
                Some("Подписки".into()),
                format!(
                    "Режим подписок не изменён: в «Тарифах» выбрана одна подписка у клиента, а {}. \
                     Новые подписки у клиентов не появятся, лишние нельзя будет продлить отдельно. \
                     Включите «Разрешить несколько подписок» в «Тарифах» или перенесите ещё раз с отметкой",
                    multi::reason(setting, clients)
                ),
            );
        }
        if old.wanted()
            && let Some(limit) = old.gift_limit
        {
            self.issue(
                "multi_gift_limit",
                "setting",
                None,
                Some("Подписки".into()),
                format!(
                    "В старой панели подарок не активировался, если у клиента уже {limit} подписок или больше. \
                     В новой панели общего предела нет: сколько подписок одной категории может быть у клиента, \
                     задаётся у категории в «Тарифах»"
                ),
            );
        }
        if !self.services.settings().multi_subscriptions {
            let limited: Vec<(String, String)> = self
                .legacy
                .categories
                .iter()
                .filter(|c| self.legacy.category_limits.contains_key(&c.old_id))
                .map(|c| (c.old_id.clone(), c.name.clone()))
                .collect();
            for (old_id, name) in limited {
                self.issue(
                    "category_limit",
                    "category",
                    Some(&old_id),
                    Some(name.clone()),
                    format!(
                        "Категория «{name}»: в старой панели — одна подписка на клиента. Перенесено как лимит в одну подписку \
                         этой категории, но он действует, только когда в «Тарифах» разрешено несколько подписок у клиента, \
                         а сейчас там выбрана одна"
                    ),
                );
            }
        }
        Ok(())
    }

    fn mail_issue(&mut self, code: &str, details: String) {
        self.issue(code, "setting", None, Some("Почта".into()), details);
    }

    async fn mail_partial(&mut self, incoming: String, reason: String) -> ServiceResult<()> {
        let mail::Old::Ready(old) = mail::old_mail(&self.legacy.settings) else {
            return Ok(());
        };
        let Some(actor) = self.actor().await? else {
            self.mail_issue(
                "mail_skipped",
                "В установке нет владельца — почта не перенесена. Создайте владельца и перенесите снова".into(),
            );
            return Ok(());
        };
        let current = self.services.modules().config(ModuleId::Mail);
        let config: Map<String, Value> = old
            .config
            .into_iter()
            .filter(|(key, _)| current.text(key).is_none_or(|v| v.trim().is_empty()))
            .collect();
        if !config.is_empty() {
            let update = ModuleConfigUpdate {
                config,
                clear: Vec::new(),
            };
            if let Err(err) = self
                .services
                .settings_ops()
                .update_module_config(ModuleId::Mail, &update, actor)
                .await
            {
                self.mail_issue(
                    "mail_skipped",
                    format!("Почта не перенесена: {err}. Настройте её в «Настройки → Почта и уведомления»"),
                );
                return Ok(());
            }
            self.done.settings += 1;
        }
        let mut conn = self.services.store().conn().await?;
        self.remember(&mut conn, "mail", "applied", self.run_id)
            .await?;
        self.mail_issue(
            "mail_partial",
            format!(
                "В старой панели письма не отправлялись: {reason}. Перенесено в пустые поля: {incoming}. \
                 Модуль «Почта» не включён — допишите настройки в «Настройки → Почта и уведомления»"
            ),
        );
        Ok(())
    }

    async fn mail(&mut self) -> ServiceResult<()> {
        let entity = self.entity("mail");
        let Some(plan) = mail::plan(self.services, &entity, &self.legacy.settings).await? else {
            return Ok(());
        };
        let replace = self.replace_mail().await?;
        let fresh = match plan {
            MailPlan::Same { .. } => return Ok(()),
            MailPlan::Skipped { reason } => {
                self.mail_issue(
                    "mail_skipped",
                    format!("Почта не перенесена: {reason}. Настройте её в «Настройки → Почта и уведомления»"),
                );
                return Ok(());
            }
            MailPlan::Differs { current, incoming } if !replace => {
                let now = if current.starts_with("отправка") {
                    "Почта не включена заново: после прошлого переноса отправку писем выключили"
                        .to_string()
                } else {
                    format!("Почта не заменена: в этой установке уже настроена {current}")
                };
                self.mail_issue(
                    "mail_kept",
                    format!(
                        "{now}. В старой панели — {incoming}. Чтобы взять старые настройки, \
                         перенесите ещё раз с отметкой «Заменить настройками старой панели» \
                         или задайте их в «Настройки → Почта и уведомления»"
                    ),
                );
                return Ok(());
            }
            MailPlan::Partial { incoming, reason } => {
                return self.mail_partial(incoming, reason).await;
            }
            MailPlan::Differs { .. } => false,
            MailPlan::Transfer { .. } => true,
        };
        let mail::Old::Ready(old) = mail::old_mail(&self.legacy.settings) else {
            return Ok(());
        };
        let Some(actor) = self.actor().await? else {
            self.mail_issue(
                "mail_skipped",
                "В установке нет владельца — почта не перенесена. Создайте владельца и перенесите снова".into(),
            );
            return Ok(());
        };
        let mut config = old.config.clone();
        let mut clear: Vec<String> = Vec::new();
        if fresh {
            if !mail::has_login(self.services) {
                config.insert("login".into(), Value::String("password".into()));
            }
        } else {
            let unused = if old.resend {
                mail::SMTP_FIELDS
            } else {
                mail::RESEND_FIELDS
            };
            clear.extend(unused.iter().map(|f| f.to_string()));
            for field in ["username", "password", "from_name"] {
                if !config.contains_key(field) && !clear.iter().any(|c| c == field) {
                    clear.push(field.to_string());
                }
            }
        }
        let ops = self.services.settings_ops();
        let update = ModuleConfigUpdate { config, clear };
        let result = match ops
            .update_module_config(ModuleId::Mail, &update, actor)
            .await
        {
            Ok(_) => ops
                .toggle_module(ModuleId::Mail, true, actor)
                .await
                .map(|_| ()),
            Err(err) => Err(err),
        };
        match result {
            Ok(()) => {
                self.done.settings += 1;
                self.done.mail = Some(old.summary.clone());
                let mut conn = self.services.store().conn().await?;
                self.remember(&mut conn, "mail", "applied", self.run_id)
                    .await?;
            }
            Err(err) => self.mail_issue(
                "mail_skipped",
                format!(
                    "Почта не перенесена: {err}. Настройте её в «Настройки → Почта и уведомления»"
                ),
            ),
        }
        Ok(())
    }

    fn invoice_intent(&self, invoice: &LInvoice) -> (PurchaseIntent, Option<i64>, String) {
        let money = Money::new(invoice.amount_minor, invoice.currency);
        let topup = |label: String| (PurchaseIntent::TopUp { amount: money }, None, label);
        match &invoice.purpose {
            InvoicePurpose::TopUp => topup("Пополнение баланса".into()),
            InvoicePurpose::Other(what) => topup(format!("{what}: оплата пойдёт на баланс")),
            InvoicePurpose::Tariff {
                tariff_old_id,
                period_old_id,
                days,
                subscription_old_id,
            } => {
                let Some(tariff) = self.tariffs.get(tariff_old_id).copied() else {
                    return topup("Тариф не перенесён: оплата пойдёт на баланс".into());
                };
                let period = period_old_id
                    .as_ref()
                    .and_then(|p| self.periods.get(p))
                    .copied()
                    .or_else(|| days.and_then(|d| self.periods_by_days.get(&(tariff, d)).copied()))
                    .or_else(|| self.period_for(tariff, days.unwrap_or(30)).map(|(_, p)| p));
                let Some(period) = period else {
                    return topup("Период тарифа не перенесён: оплата пойдёт на баланс".into());
                };
                let target = subscription_old_id
                    .as_ref()
                    .and_then(|s| self.subs.get(s))
                    .copied();
                let name = self
                    .tariff_names
                    .get(&tariff)
                    .cloned()
                    .unwrap_or_else(|| "Тариф".into());
                (
                    PurchaseIntent::Subscription {
                        tariff_id: TariffId(tariff),
                        period_id: PeriodId(period),
                        extra_devices: 0,
                        target: target.map(SubscriptionId),
                    },
                    target,
                    format!("Тариф «{name}»"),
                )
            }
        }
    }

    async fn invoices(&mut self) -> ServiceResult<()> {
        let legacy = self.legacy;
        let source = legacy.source.as_str();
        let keep: Vec<&str> = legacy.invoices.iter().map(|i| i.old_id.as_str()).collect();
        let mut tx = self.services.store().begin().await?;
        sqlx::query(
            "DELETE FROM legacy_invoices WHERE source = $1 AND payment_id IS NULL AND NOT (old_id = ANY($2))",
        )
        .bind(source)
        .bind(&keep)
        .execute(tx.conn())
        .await?;
        for invoice in &legacy.invoices {
            let Some(client) = self.clients.get(&invoice.client_old_id).copied() else {
                continue;
            };
            let (intent, target, label) = self.invoice_intent(invoice);
            let intent =
                serde_json::to_value(&intent).map_err(|e| ServiceError::Internal(e.to_string()))?;
            sqlx::query(
                "INSERT INTO legacy_invoices (source, old_id, provider, old_provider, external_id, order_ref, client_id, \
                 intent, label, amount_minor, currency, stars, pay_url, created_at, run_id, recurring) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
                 ON CONFLICT (source, old_id) DO UPDATE SET provider = EXCLUDED.provider, old_provider = EXCLUDED.old_provider, \
                 external_id = EXCLUDED.external_id, order_ref = EXCLUDED.order_ref, client_id = EXCLUDED.client_id, \
                 intent = EXCLUDED.intent, label = EXCLUDED.label, amount_minor = EXCLUDED.amount_minor, \
                 currency = EXCLUDED.currency, stars = EXCLUDED.stars, pay_url = EXCLUDED.pay_url, \
                 run_id = EXCLUDED.run_id, recurring = EXCLUDED.recurring \
                 WHERE legacy_invoices.payment_id IS NULL",
            )
            .bind(source)
            .bind(&invoice.old_id)
            .bind(invoice.provider)
            .bind(&invoice.old_provider)
            .bind(&invoice.external_id)
            .bind(&invoice.order_ref)
            .bind(client)
            .bind(&intent)
            .bind(match (target, invoice.recurring) {
                (_, true) => format!("{label}: подписка Lava.top с ежемесячным списанием"),
                (Some(_), false) => format!("{label}, продление"),
                (None, false) => label,
            })
            .bind(invoice.amount_minor)
            .bind(invoice.currency.code())
            .bind(invoice.stars.and_then(|s| i32::try_from(s).ok()))
            .bind(&invoice.pay_url)
            .bind(invoice.created_at)
            .bind(self.run_id)
            .bind(invoice.recurring)
            .execute(tx.conn())
            .await?;
            if !invoice.old_id.starts_with("contract:") {
                self.done.invoices += 1;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn settings(&mut self, actor: AdminId) {
        let legacy = self.legacy;
        let services = self.services;
        let old = &legacy.settings;
        let ops = services.settings_ops();
        let present = self.present().await.unwrap_or_default();
        let unset = |key: &str| !present.contains(key);
        let brand = value(old, "service_name")
            .or_else(|| value(old, "site_name"))
            .map(|s| s.chars().take(64).collect::<String>())
            .filter(|_| unset("core.brand_name"));
        let support = value(old, "support_link")
            .or_else(|| value(old, "support_bot_username"))
            .and_then(support_url)
            .filter(|_| unset("core.support_url") && unset("core.support_mode"));
        if brand.is_some() || support.is_some() {
            let tickets = value(old, "tickets_enabled").is_none_or(truthy);
            let mode = match (&support, tickets) {
                (Some(_), true) => SupportMode::Both,
                (Some(_), false) => SupportMode::Link,
                (None, _) => SupportMode::Tickets,
            };
            let update = CoreUpdate {
                support_mode: support.is_some().then_some(mode),
                brand_name: brand,
                support_url: support,
                ..CoreUpdate::default()
            };
            let result = ops.update_core(&update, actor).await.map(|_| ());
            self.outcome("Название сервиса и поддержка", result);
        }
        let days = value(old, "trial_days")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let squad = value(old, "trial_squad_uuid").and_then(|v| uuid::Uuid::parse_str(v).ok());
        if let (true, Some(squad), true) = (days > 0, squad, unset("billing.trial")) {
            let devices = value(old, "trial_device_limit")
                .or_else(|| value(old, "trial_devices"))
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|d| *d > 0)
                .unwrap_or(1)
                .min(100);
            let limit = value(old, "trial_traffic_limit")
                .or_else(|| value(old, "trial_traffic_limit_bytes"))
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|b| *b > 0);
            let trial = TrialSettings::single(
                true,
                days.min(90),
                vec![SquadId(squad)],
                TrafficPolicy {
                    limit_bytes: limit,
                    reset: TrafficReset::NoReset,
                },
                devices,
            );
            let result = ops.update_trial(&trial, actor).await.map(|_| ());
            self.outcome("Пробный период", result);
        }
        let default = match value(old, "default_language") {
            Some(l) if l.eq_ignore_ascii_case("en") => Locale::En,
            _ => Locale::Ru,
        };
        let english = value(old, "active_languages").is_some_and(|l| {
            l.to_ascii_lowercase()
                .split(|c: char| !c.is_ascii_alphabetic())
                .any(|p| p == "en")
        }) || default == Locale::En;
        if english && unset("core.languages") {
            let result = ops
                .update_languages(&Languages { english, default }, actor)
                .await
                .map(|_| ());
            self.outcome("Языки", result);
        }
        let mut documents: Vec<ListItem> = Vec::new();
        for (key, title) in [
            ("offer_link", "Публичная оферта"),
            ("landing_offer_link", "Публичная оферта"),
            ("agreement_link", "Пользовательское соглашение"),
            ("landing_privacy_link", "Политика конфиденциальности"),
            ("instructions_link", "Инструкция по подключению"),
        ] {
            let Some(url) = value(old, key) else { continue };
            let url = if url.starts_with("http://") || url.starts_with("https://") {
                url.to_string()
            } else {
                continue;
            };
            if documents
                .iter()
                .any(|d| d.url.as_deref() == Some(url.as_str()) || d.title == title)
            {
                continue;
            }
            documents.push(ListItem {
                title: title.to_string(),
                url: Some(url),
                ..ListItem::default()
            });
        }
        if !documents.is_empty() && unset("bot.documents") {
            let update = ListUpdate {
                consent: false,
                items: documents,
            };
            let result = services
                .documents()
                .replace(&update, Some(actor))
                .await
                .map(|_| ());
            self.outcome("Документы: оферта, соглашение, инструкция", result);
        }
        self.bot_content(actor).await;
        let taken = self.module_fields("referral").await.unwrap_or_default();
        let mut referral = Map::new();
        for (field, keys) in [
            (
                "level1_percent",
                ["default_referral_percent", "referral_level1_percent"],
            ),
            (
                "level2_percent",
                ["referral_percent_level_2", "referral_level2_percent"],
            ),
            (
                "level3_percent",
                ["referral_percent_level_3", "referral_level3_percent"],
            ),
        ] {
            if taken.contains(field) {
                continue;
            }
            if let Some(percent) = keys
                .iter()
                .find_map(|k| value(old, k))
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
            {
                referral.insert(field.into(), Value::String(format!("{}", percent.round())));
            }
        }
        if !referral.is_empty() {
            let update = ModuleConfigUpdate {
                config: referral,
                clear: Vec::new(),
            };
            let result = ops
                .update_module_config(ModuleId::Referral, &update, actor)
                .await
                .map(|_| ());
            self.outcome("Проценты реферальной программы", result);
        }
        let gifts_on = value(old, "gift_subscriptions_enabled").is_some_and(truthy)
            || !legacy.gifts.is_empty();
        if gifts_on {
            let result = ops
                .toggle_module(ModuleId::Gifts, true, actor)
                .await
                .map(|_| ());
            self.outcome("Модуль подарков", result);
        }
        for (key, what) in [
            ("bot_welcome_text", "Приветствие бота"),
            ("bot_info_block", "Блок информации в меню"),
            ("bot_tariffs_text", "Текст списка тарифов"),
            ("bot_payment_text", "Текст оплаты"),
        ] {
            if let Some(text) = value(old, key) {
                self.issue(
                    "setting_text",
                    "setting",
                    None,
                    Some(what.to_string()),
                    format!(
                        "В новом боте другое устройство экранов, текст не перенесён. Если нужен — вставьте в «Бот → Тексты»: {}",
                        text.chars().take(1500).collect::<String>()
                    ),
                );
            }
        }
    }

    async fn bot_content(&mut self, actor: AdminId) {
        let legacy = self.legacy;
        let services = self.services;
        let old = &legacy.settings;
        let emojis = json(value(old, "bot_emojis"));
        let current = services.bot_content().content.config().clone();
        let mut update = ContentUpdate::default();
        let mut problems: Vec<(String, String)> = Vec::new();
        if let Some(columns) = value(old, "bot_buttons_per_row").and_then(|v| v.parse::<u8>().ok())
            && current.menu_columns.is_none()
        {
            update.menu_columns = Some(if columns >= 2 { 2 } else { 1 });
        }
        if let Some(Value::Array(buttons)) = json(value(old, "bot_buttons")) {
            for button in &buttons {
                let Some(id) = button.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let keys = button_keys(id);
                if keys.is_empty() {
                    continue;
                }
                let (symbol, emoji_id) = button
                    .get("emojiKey")
                    .and_then(Value::as_str)
                    .map(|k| emoji_of(&emojis, k))
                    .unwrap_or((None, None));
                let label = button
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| match (&symbol, &emoji_id) {
                        (Some(symbol), None) if !l.contains(symbol.as_str()) => {
                            format!("{symbol} {l}")
                        }
                        _ => l.to_string(),
                    });
                let hidden = button.get("visible").and_then(Value::as_bool) == Some(false);
                for (n, key) in keys.iter().enumerate() {
                    if update.buttons.contains_key(*key) || current.buttons.contains_key(*key) {
                        continue;
                    }
                    let raw = ButtonOverride {
                        text: if n == 0 { label.clone() } else { None },
                        emoji_id: emoji_id.clone(),
                        style: style_of(button.get("style").and_then(Value::as_str)),
                        hidden,
                    };
                    match clean_button(key, &raw) {
                        Ok(clean) if !clean.is_empty() => {
                            update.buttons.insert((*key).to_string(), Some(clean));
                        }
                        Ok(_) => {}
                        Err(_) if hidden => {
                            let shown = ButtonOverride {
                                hidden: false,
                                ..raw
                            };
                            if let Ok(clean) = clean_button(key, &shown)
                                && !clean.is_empty()
                            {
                                update.buttons.insert((*key).to_string(), Some(clean));
                            }
                            problems.push((
                                format!("Кнопка «{id}»"),
                                "в старом боте была скрыта, а в новом её скрыть нельзя".into(),
                            ));
                        }
                        Err(err) => problems.push((format!("Кнопка «{id}»"), err.message)),
                    }
                }
            }
        }
        if let Some(label) = value(old, "bot_back_label") {
            let key = if label.to_lowercase().contains("меню") {
                "common.btn.menu"
            } else {
                "common.btn.back"
            };
            let key = if current.buttons.contains_key(key) {
                ""
            } else {
                key
            };
            let raw = ButtonOverride {
                text: Some(old_text(label, &emojis)),
                ..ButtonOverride::default()
            };
            match clean_button(key, &raw) {
                Ok(clean) if !clean.is_empty() => {
                    update.buttons.insert(key.to_string(), Some(clean));
                }
                Ok(_) => {}
                Err(_) if key.is_empty() => {}
                Err(err) => problems.push(("Кнопка «Назад»".into(), err.message)),
            }
        }
        let texts = json(value(old, "bot_menu_texts"));
        let text = |name: &str| {
            texts
                .as_ref()
                .and_then(|t| t.get(name))
                .and_then(Value::as_str)
                .map(|s| old_text(s, &emojis))
                .filter(|s| !s.is_empty())
        };
        let mut wanted: Vec<(&str, String)> = Vec::new();
        if let Some(greeting) = text("welcomeGreeting") {
            wanted.push(("menu.greeting", greeting));
        }
        if let Some(prefix) = text("balancePrefix") {
            wanted.push(("menu.balance", format!("{prefix} {{balance}}")));
        }
        if let Some(Value::Object(visible)) = json(value(old, "bot_menu_line_visibility")) {
            for (name, keys) in [
                ("balance", &["menu.balance"][..]),
                ("balancePrefix", &["menu.balance"][..]),
                (
                    "traffic",
                    &["menu.single_traffic", "menu.single_traffic_unlimited"][..],
                ),
                (
                    "trafficPrefix",
                    &["menu.single_traffic", "menu.single_traffic_unlimited"][..],
                ),
                (
                    "devices",
                    &["menu.single_devices", "menu.single_devices_limit"][..],
                ),
                (
                    "devicesLabel",
                    &["menu.single_devices", "menu.single_devices_limit"][..],
                ),
            ] {
                if visible.get(name).and_then(Value::as_bool) == Some(false) {
                    for key in keys {
                        wanted.retain(|(k, _)| k != key);
                        wanted.push((key, String::new()));
                    }
                }
            }
        }
        for (key, raw) in wanted {
            if current.texts.contains_key(key) {
                continue;
            }
            match clean_text(key, &raw) {
                Ok(clean) => {
                    update.texts.insert(key.to_string(), Some(clean));
                }
                Err(err) => problems.push((format!("Текст «{key}»"), err.message)),
            }
        }
        for (what, why) in problems {
            self.issue(
                "setting_skipped",
                "setting",
                None,
                Some(what),
                format!("Не перенесено: {why}"),
            );
        }
        if update.texts.is_empty() && update.buttons.is_empty() && update.menu_columns.is_none() {
            return;
        }
        let result = services
            .bot_content_ops()
            .update_content(&update, actor)
            .await
            .map(|_| ());
        self.outcome("Тексты и кнопки бота", result);
    }

    async fn providers(&mut self, actor: AdminId) {
        let legacy = self.legacy;
        let services = self.services;
        let old = &legacy.settings;
        let flag = |key: &str| {
            Value::String(
                if value(old, key).is_some_and(truthy) {
                    "true"
                } else {
                    "false"
                }
                .into(),
            )
        };
        let text = |key: &str| value(old, key).map(|v| Value::String(v.to_string()));
        type Field<'k> = (&'k str, Option<Value>, bool);
        let plans: Vec<(&str, &str, Vec<Field>, bool)> = vec![
            (
                "cryptopay",
                "Crypto Pay",
                vec![
                    ("api_token", text("cryptopay_api_token"), true),
                    ("testnet", Some(flag("cryptopay_testnet")), false),
                ],
                true,
            ),
            (
                "heleket",
                "Heleket",
                vec![
                    ("merchant_id", text("heleket_merchant_id"), true),
                    ("api_key", text("heleket_api_key"), true),
                ],
                true,
            ),
            (
                "lava",
                "Lava.top",
                vec![
                    ("api_key", text("lavatop_api_key"), true),
                    ("offer_id", text("lavatop_default_offer_id"), true),
                    ("webhook_key", text("lavatop_api_key"), true),
                ],
                true,
            ),
            (
                "yookassa",
                "ЮKassa",
                vec![
                    ("shop_id", text("yookassa_shop_id"), true),
                    ("secret_key", text("yookassa_secret_key"), true),
                    (
                        "autopayments",
                        Some(flag("yookassa_recurring_enabled")),
                        false,
                    ),
                ],
                true,
            ),
            (
                "platega",
                "Platega",
                vec![
                    ("merchant_id", text("platega_merchant_id"), true),
                    ("secret", text("platega_secret"), true),
                ],
                true,
            ),
            (
                "yoomoney",
                "ЮMoney",
                vec![
                    ("wallet", text("yoomoney_receiver_wallet"), true),
                    (
                        "notification_secret",
                        text("yoomoney_notification_secret"),
                        true,
                    ),
                ],
                true,
            ),
            (
                "rollypay",
                "RollyPay",
                vec![
                    ("api_key", text("rollypay_api_key"), true),
                    ("signing_secret", text("rollypay_signing_secret"), true),
                ],
                true,
            ),
            (
                "overpay",
                "Overpay",
                vec![
                    ("api_url", text("overpay_api_url"), true),
                    ("project_id", text("overpay_project_id"), true),
                    ("login", text("overpay_login"), true),
                    ("password", text("overpay_password"), true),
                ],
                false,
            ),
            (
                "paritypay",
                "ParityPay",
                vec![
                    ("shop_id", text("paritypay_shop_id"), true),
                    ("secret_key", text("paritypay_api_key"), true),
                    ("callback_secret", text("paritypay_signing_secret"), true),
                ],
                true,
            ),
        ];
        let registry = services.payments();
        let present = self.present().await.unwrap_or_default();
        for (id, title, fields, enable) in plans {
            if registry.get(id).is_none() {
                continue;
            }
            if present.contains(&sn_domain::settings::keys::provider(id)) {
                if fields
                    .iter()
                    .any(|(_, v, required)| *required && v.is_some())
                {
                    self.issue(
                        "provider_kept",
                        "setting",
                        None,
                        Some(title.to_string()),
                        "Платёжка уже настроена в новой панели — ключи из старой не переносим",
                    );
                }
                continue;
            }
            if !fields
                .iter()
                .any(|(_, v, required)| *required && v.is_some())
            {
                continue;
            }
            let missing: Vec<&str> = fields
                .iter()
                .filter(|(_, v, required)| *required && v.is_none())
                .map(|(k, _, _)| *k)
                .collect();
            let mut config = Map::new();
            for (key, v, _) in fields {
                if let Some(v) = v {
                    config.insert(key.to_string(), v);
                }
            }
            let methods = if id == "platega" {
                platega_methods(value(old, "platega_methods"))
            } else {
                None
            };
            let on = enable && missing.is_empty();
            let update = ProviderUpdate {
                topup_bonus: None,
                enabled: Some(on),
                config: config.clone(),
                clear: Vec::new(),
                channels: None,
                methods: methods.clone(),
                label: None,
            };
            let ops = services.settings_ops();
            match ops.update_provider(id, &update, actor).await {
                Ok(_) if on => {
                    self.done.providers.push(title.to_string());
                    self.done.settings += 1;
                }
                Ok(_) => {
                    self.done.settings += 1;
                    self.issue(
                        "provider_off",
                        "setting",
                        None,
                        Some(title.to_string()),
                        if missing.is_empty() {
                            "Ключи перенесены, но платёжка выключена: в новой системе ей нужен сертификат. Добавьте его и включите".to_string()
                        } else {
                            format!(
                                "Ключи перенесены не все (нет: {}), платёжка выключена. Допишите и включите",
                                missing.join(", ")
                            )
                        },
                    );
                }
                Err(err) => {
                    let saved = ops
                        .update_provider(
                            id,
                            &ProviderUpdate {
                                enabled: Some(false),
                                ..update
                            },
                            actor,
                        )
                        .await
                        .is_ok();
                    self.issue(
                        "provider_off",
                        "setting",
                        None,
                        Some(title.to_string()),
                        format!(
                            "Платёжка не включилась: {err}. {}",
                            if saved {
                                "Ключи сохранены, проверьте их и включите вручную"
                            } else {
                                "Введите ключи вручную"
                            }
                        ),
                    );
                }
            }
        }
    }

    async fn admins(&mut self) -> ServiceResult<()> {
        let legacy = self.legacy;
        if legacy.admins.is_empty() {
            return Ok(());
        }
        let known = self.known("admin").await?;
        for admin in &legacy.admins {
            if known.contains_key(&admin.old_id) {
                continue;
            }
            let mut tx = self.services.store().begin().await?;
            if let Some(existing) = tx.admins().by_email(&admin.email).await? {
                self.remember(&mut tx, "admin", &admin.old_id, existing.id.0)
                    .await?;
                tx.commit().await?;
                self.issue(
                    "admin_exists",
                    "admin",
                    Some(&admin.old_id),
                    Some(admin.email.clone()),
                    "Админ с этой почтой уже есть в новой панели — оставлен как есть",
                );
                continue;
            }
            let (role, permissions) = permissions_of(admin);
            let created = tx
                .admins()
                .create(&sn_store::NewAdmin {
                    email: admin.email.clone(),
                    password_hash: admin.password_hash.clone(),
                    role,
                    must_change_password: false,
                })
                .await?;
            if !permissions.is_empty() {
                tx.admins().set_role(created.id, role, &permissions).await?;
            }
            self.remember(&mut tx, "admin", &admin.old_id, created.id.0)
                .await?;
            tx.commit().await?;
            self.done.admins += 1;
        }
        Ok(())
    }

    async fn withdrawals(&mut self) -> ServiceResult<()> {
        let legacy = self.legacy;
        if legacy.withdrawals.is_empty() {
            return Ok(());
        }
        let known = self.known("withdrawal").await?;
        let currency = legacy.currency.code();
        for item in &legacy.withdrawals {
            let Some(client) = self.clients.get(&item.client_old_id).copied() else {
                continue;
            };
            let mut tx = self.services.store().begin().await?;
            if let Some(id) = known.get(&item.old_id) {
                if item.state != WithdrawalState::Pending {
                    sqlx::query(
                        "UPDATE referral_withdrawals SET status = $2, decided_at = COALESCE($3, now()), \
                         released_at = CASE WHEN $2 = 'rejected' THEN COALESCE($3, now()) END, comment = COALESCE($4, comment) \
                         WHERE id = $1 AND status = 'pending'",
                    )
                    .bind(*id)
                    .bind(state_word(item.state))
                    .bind(item.decided_at)
                    .bind(&item.comment)
                    .execute(tx.conn())
                    .await?;
                }
                tx.commit().await?;
                continue;
            }
            let id = insert_withdrawal(&mut tx, item, client, currency).await?;
            self.remember(&mut tx, "withdrawal", &item.old_id, id)
                .await?;
            tx.commit().await?;
            self.done.withdrawals += 1;
        }
        Ok(())
    }

    async fn gifts(&mut self) -> ServiceResult<()> {
        let legacy = self.legacy;
        if legacy.gifts.is_empty() {
            return Ok(());
        }
        let known = self.known("gift").await?;
        for gift in &legacy.gifts {
            if known.contains_key(&gift.old_id) {
                continue;
            }
            let who = legacy
                .clients
                .iter()
                .find(|c| c.old_id == gift.giver_old_id)
                .map(super::model::LClient::who);
            let Some(code) = gifts::normalize_code(&gift.code) else {
                self.issue(
                    "gift_skipped",
                    "gift",
                    Some(&gift.old_id),
                    who,
                    format!(
                        "Код {} не похож на подарочный код — не перенесён",
                        gift.code
                    ),
                );
                continue;
            };
            let Some(giver) = self.clients.get(&gift.giver_old_id).copied() else {
                continue;
            };
            let tariff = gift
                .tariff_old_id
                .as_ref()
                .and_then(|t| self.tariffs.get(t))
                .copied();
            let Some((tariff, period)) = tariff.and_then(|t| self.period_for(t, gift.days)) else {
                self.issue(
                    "gift_skipped",
                    "gift",
                    Some(&gift.old_id),
                    who,
                    format!(
                        "Подарок {code}: тариф подписки не перенесён — выдайте получателю вручную"
                    ),
                );
                continue;
            };
            let name = self
                .tariff_names
                .get(&tariff)
                .cloned()
                .unwrap_or_else(|| "Подарок".into());
            let mut tx = self.services.store().begin().await?;
            let order = gift_order(&mut tx, legacy.source, gift, giver, legacy.currency).await?;
            let inserted = gifts::store::insert(
                &mut tx,
                &gifts::store::NewGift {
                    code: &code,
                    order_id: OrderId(order),
                    giver_id: ClientId(giver),
                    tariff_id: TariffId(tariff),
                    period_id: PeriodId(period),
                    days: gift.days,
                    tariff_name: &name,
                    expires_at: gift.expires_at,
                },
            )
            .await;
            match inserted {
                Ok(Some(id)) => {
                    self.remember(&mut tx, "gift", &gift.old_id, id).await?;
                    tx.commit().await?;
                    self.done.gifts += 1;
                }
                Ok(None) | Err(_) => {
                    drop(tx);
                    self.issue(
                        "gift_skipped",
                        "gift",
                        Some(&gift.old_id),
                        who,
                        format!("Подарок {code} не перенесён: такой код уже есть в новой системе"),
                    );
                }
            }
        }
        Ok(())
    }

    fn period_for(&self, tariff: i64, days: u32) -> Option<(i64, i64)> {
        self.periods_by_days
            .iter()
            .filter(|((t, _), _)| *t == tariff)
            .min_by_key(|((_, d), _)| d.abs_diff(days))
            .map(|(_, id)| (tariff, *id))
    }

    async fn rules(&mut self) -> ServiceResult<()> {
        let legacy = self.legacy;
        if legacy.auto_rules.is_empty() {
            return Ok(());
        }
        let known = self.known("auto_rule").await?;
        let ctx = self.services.module_context();
        for rule in &legacy.auto_rules {
            if known.contains_key(&rule.old_id) {
                continue;
            }
            let title = rule.name.clone().unwrap_or_else(|| rule.trigger.clone());
            let Some((trigger, days)) = trigger_of(rule) else {
                self.issue(
                    "rule_skipped",
                    "auto_rule",
                    Some(&rule.old_id),
                    Some(title),
                    format!(
                        "Авторассылка «{}» не перенесена: такого повода в новой системе нет",
                        rule.trigger
                    ),
                );
                continue;
            };
            let text = clean_rule_text(&rule.text);
            let buttons: Vec<ButtonSpec> = rule
                .buttons
                .iter()
                .filter_map(|(label, url)| rule_button(label, url))
                .collect();
            let dropped = rule.buttons.len() - buttons.len();
            let draft = RuleDraft {
                trigger,
                amount: days,
                unit: Unit::Days,
                percent: None,
                with_autorenew: false,
                channel: match rule.channel.as_str() {
                    "email" => Channel::Email,
                    "both" => Channel::Both,
                    _ => Channel::Bot,
                },
                subject: rule.subject.clone(),
                text: text.clone(),
                image_id: None,
                buttons,
                en: None,
                enabled: false,
            };
            match create_rule(&ctx, &draft, self.admin).await {
                Ok(created) => {
                    let mut conn = self.services.store().conn().await?;
                    self.remember(&mut conn, "auto_rule", &rule.old_id, created.id)
                        .await?;
                    self.done.rules += 1;
                    let mut notes = Vec::new();
                    if text != rule.text.trim() {
                        notes.push("подстановки промокода и скидки из старого текста убраны");
                    }
                    if dropped > 0 {
                        notes.push("кнопка со ссылкой, которой нет в новом боте, убрана");
                    }
                    if !notes.is_empty() {
                        self.issue(
                            "rule_changed",
                            "auto_rule",
                            Some(&rule.old_id),
                            Some(title),
                            format!("Перенесена выключенной; {} — проверьте", notes.join(", ")),
                        );
                    }
                }
                Err(err) => self.issue(
                    "rule_skipped",
                    "auto_rule",
                    Some(&rule.old_id),
                    Some(title),
                    format!("Авторассылка не перенесена: {err}"),
                ),
            }
        }
        Ok(())
    }
}

fn rule_button(label: &str, url: &str) -> Option<ButtonSpec> {
    let url = url.trim();
    let text = label.trim().to_string();
    if url.contains("{{") || text.is_empty() {
        return None;
    }
    if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("tg://") {
        return Some(ButtonSpec {
            text,
            url: Some(url.to_string()),
            section: None,
        });
    }
    let section = match url.strip_prefix("menu:").unwrap_or(url).trim_matches('/') {
        "tariffs" | "buy" | "extend" | "renew" => BotSection::Buy,
        "trial" => BotSection::Trial,
        "topup" | "balance" => BotSection::Balance,
        "support" | "tickets" => BotSection::Support,
        "promocode" | "promo" => BotSection::Promo,
        "referral" => BotSection::Referral,
        "extra_options" | "options" => BotSection::Options,
        "vpn" | "my_subs" | "subscriptions" | "subscription" => BotSection::Subscriptions,
        "contests" => BotSection::Contests,
        "" | "menu" | "main" | "start" => BotSection::Menu,
        _ => return None,
    };
    Some(ButtonSpec {
        text,
        url: None,
        section: Some(section),
    })
}

fn clean_rule_text(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find("{{") {
        out.push_str(&rest[..at]);
        match rest[at + 2..].find("}}") {
            Some(end) => rest = &rest[at + 2 + end + 2..],
            None => {
                out.push_str(&rest[at..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

fn platega_methods(raw: Option<&str>) -> Option<Vec<sn_domain::payment::MethodChoice>> {
    let Value::Array(items) = json(raw)? else {
        return None;
    };
    let mut out = Vec::new();
    for item in items {
        if item.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let code = match item.get("id").and_then(Value::as_i64) {
            Some(2) => "sbp",
            Some(11) => "card",
            Some(12) => "intl_card",
            Some(13) => "crypto",
            Some(14) => "sberpay",
            _ => continue,
        };
        let title = item
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        out.push(sn_domain::payment::MethodChoice {
            code: code.to_string(),
            title,
        });
    }
    (!out.is_empty()).then_some(out)
}

fn state_word(state: WithdrawalState) -> &'static str {
    match state {
        WithdrawalState::Pending => "pending",
        WithdrawalState::Paid => "paid",
        WithdrawalState::Rejected => "rejected",
    }
}

async fn insert_withdrawal(
    db: &mut impl Db,
    item: &LWithdrawal,
    client: i64,
    currency: &str,
) -> ServiceResult<i64> {
    let decided = item.decided_at;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO referral_withdrawals (client_id, amount_minor, currency, method, method_title, details, status, \
         comment, held_at, released_at, decided_at, created_at) \
         VALUES ($1, $2, $3, 'usdt_trc20', 'USDT (TRC20)', $4, $5, $6, COALESCE($7, now()), \
         CASE WHEN $5 = 'rejected' THEN COALESCE($8, $7, now()) END, \
         CASE WHEN $5 = 'pending' THEN NULL ELSE COALESCE($8, $7, now()) END, COALESCE($7, now())) RETURNING id",
    )
    .bind(client)
    .bind(item.amount_minor)
    .bind(currency)
    .bind(&item.details)
    .bind(state_word(item.state))
    .bind(&item.comment)
    .bind(item.created_at)
    .bind(decided)
    .fetch_one(db.conn())
    .await?;
    Ok(id)
}

async fn gift_order(
    db: &mut impl Db,
    source: Source,
    gift: &LGift,
    giver: i64,
    currency: sn_domain::money::Currency,
) -> ServiceResult<i64> {
    let intent = PurchaseIntent::Imported {
        source: source.as_str().to_string(),
        old_id: format!("gift:{}", gift.old_id),
        label: Some("Подарок из старой панели".into()),
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO orders (client_id, kind, status, intent, quote, currency, total_minor, created_at, paid_at, fulfilled_at) \
         VALUES ($1, 'module', 'fulfilled', $2, '{}'::jsonb, $3, 0, COALESCE($4, now()), COALESCE($4, now()), COALESCE($4, now())) RETURNING id",
    )
    .bind(giver)
    .bind(sqlx::types::Json(&intent))
    .bind(currency.code())
    .bind(gift.created_at)
    .fetch_one(db.conn())
    .await
    .map_err(ServiceError::from)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ссылка_поддержки_приводится_к_адресу() {
        assert_eq!(
            support_url("@help_me").as_deref(),
            Some("https://t.me/help_me")
        );
        assert_eq!(support_url("t.me/x").as_deref(), Some("https://t.me/x"));
        assert_eq!(
            support_url("http://a.b/c").as_deref(),
            Some("https://a.b/c")
        );
        assert_eq!(
            support_url("help_bot").as_deref(),
            Some("https://t.me/help_bot")
        );
        assert_eq!(support_url("пишите нам"), None);
    }

    #[test]
    fn старый_текст_очищается_от_подстановок() {
        let emojis = Some(serde_json::json!({"BALANCE": {"unicode": "💰"}, "STAR": "⭐"}));
        assert_eq!(
            old_text("{{BALANCE}} **Баланс:** {{SUB_X}}", &emojis),
            "💰 <b>Баланс:</b>"
        );
        assert_eq!(old_text("{{STAR}} Привет", &emojis), "⭐ Привет");
        assert_eq!(
            clean_rule_text("Скидка {{DISCOUNT}}% по {{PROMOCODE}}"),
            "Скидка % по"
        );
    }

    #[test]
    fn кнопки_авторассылки_ведут_в_разделы_бота() {
        let button = rule_button("Продлить", "menu:tariffs").unwrap();
        assert_eq!(button.section, Some(BotSection::Buy));
        assert_eq!(button.url, None);
        assert_eq!(
            rule_button("Сайт", "https://example.com")
                .unwrap()
                .url
                .as_deref(),
            Some("https://example.com")
        );
        assert!(rule_button("Своя", "menu:proxy").is_none());
        assert!(rule_button("Скидка", "https://x.io/?p={{PROMOCODE}}").is_none());
    }

    #[test]
    fn права_менеджера_собираются_из_разделов() {
        let admin = LAdmin {
            old_id: "a1".into(),
            email: "m@example.com".into(),
            password_hash: "$2b$10$x".into(),
            full_access: false,
            sections: vec![
                "clients".into(),
                "tickets".into(),
                "action:refund_payment".into(),
                "promo-codes".into(),
            ],
        };
        let (role, perms) = permissions_of(&admin);
        assert_eq!(role, AdminRole::ReadOnly);
        assert_eq!(
            perms,
            [
                Permission::ClientsRead,
                Permission::ClientsWrite,
                Permission::Refunds,
                Permission::Support,
                Permission::Marketing
            ]
        );
    }
}
