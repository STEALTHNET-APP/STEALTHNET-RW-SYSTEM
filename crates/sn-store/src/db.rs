use crate::error::{StoreError, StoreResult};
use crate::repo::*;
use chrono::{DateTime, Utc};
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct StoreOptions {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout: Duration,
    pub search_path: Option<String>,
    pub application_name: String,
}

impl Default for StoreOptions {
    fn default() -> Self {
        StoreOptions {
            max_connections: 16,
            min_connections: 0,
            acquire_timeout: Duration::from_secs(8),
            search_path: None,
            application_name: "stealthnet".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockSpace {
    Client = 1,
    Subscription = 2,
    Order = 3,
    AutoRenew = 4,
    Telegram = 5,
    Worker = 6,
    Category = 7,
    Documents = 8,
    SiteSignup = 9,
}

impl LockSpace {
    pub const fn key(self, id: i64) -> i64 {
        ((self as i64) << 48) | (id & 0xFFFF_FFFF_FFFF)
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    pool: PgPool,
}

fn connect_options(url: &str, options: &StoreOptions) -> StoreResult<PgConnectOptions> {
    let parsed = PgConnectOptions::from_str(url)
        .map_err(|e| StoreError::Invalid(format!("строка подключения к базе: {e}")))?;
    let mut params: Vec<(&str, &str)> = vec![("timezone", "UTC")];
    if let Some(path) = options.search_path.as_deref() {
        params.push(("search_path", path));
    }
    Ok(parsed
        .application_name(&options.application_name)
        .options(params))
}

fn pool_options(options: &StoreOptions) -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(options.max_connections)
        .min_connections(options.min_connections)
        .acquire_timeout(options.acquire_timeout)
}

impl Store {
    pub async fn connect(url: &str, options: StoreOptions) -> StoreResult<Self> {
        let pool = pool_options(&options)
            .connect_with(connect_options(url, &options)?)
            .await?;
        Ok(Store { pool })
    }

    pub fn connect_lazy(url: &str, options: StoreOptions) -> StoreResult<Self> {
        let pool = pool_options(&options).connect_lazy_with(connect_options(url, &options)?);
        Ok(Store { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Store { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> StoreResult<()> {
        crate::MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub async fn pending_migrations(&self) -> StoreResult<usize> {
        let total = crate::MIGRATOR.iter().count();
        let applied: Result<i64, sqlx::Error> =
            sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(&self.pool)
                .await;
        match applied {
            Ok(applied) => Ok(total.saturating_sub(applied as usize)),
            Err(sqlx::Error::Database(db)) if db.code().as_deref() == Some("42P01") => Ok(total),
            Err(err) => Err(err.into()),
        }
    }

    pub async fn installed_at(&self) -> StoreResult<Option<DateTime<Utc>>> {
        Ok(
            sqlx::query_scalar("SELECT min(installed_on) FROM _sqlx_migrations WHERE success")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn ping(&self) -> StoreResult<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn conn(&self) -> StoreResult<Conn> {
        Ok(Conn(self.pool.acquire().await?))
    }

    pub async fn begin(&self) -> StoreResult<Tx> {
        Ok(Tx(self.pool.begin().await?))
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

pub struct Conn(PoolConnection<Postgres>);

pub struct Tx(Transaction<'static, Postgres>);

impl Tx {
    pub async fn commit(self) -> StoreResult<()> {
        self.0.commit().await?;
        Ok(())
    }

    pub async fn rollback(self) -> StoreResult<()> {
        self.0.rollback().await?;
        Ok(())
    }

    pub async fn lock(&mut self, space: LockSpace, id: i64) -> StoreResult<()> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(space.key(id))
            .execute(&mut *self.0)
            .await?;
        Ok(())
    }

    pub async fn try_lock(&mut self, space: LockSpace, id: i64) -> StoreResult<bool> {
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(space.key(id))
            .fetch_one(&mut *self.0)
            .await?;
        Ok(locked)
    }
}

pub trait Db: Send {
    fn conn(&mut self) -> &mut PgConnection;

    fn settings(&mut self) -> SettingsRepo<'_> {
        SettingsRepo::new(self.conn())
    }

    fn secrets(&mut self) -> SecretsRepo<'_> {
        SecretsRepo::new(self.conn())
    }

    fn site_assets(&mut self) -> SiteAssetsRepo<'_> {
        SiteAssetsRepo::new(self.conn())
    }

    fn admins(&mut self) -> AdminsRepo<'_> {
        AdminsRepo::new(self.conn())
    }

    fn api_keys(&mut self) -> ApiKeysRepo<'_> {
        ApiKeysRepo::new(self.conn())
    }

    fn sessions(&mut self) -> SessionsRepo<'_> {
        SessionsRepo::new(self.conn())
    }

    fn clients(&mut self) -> ClientsRepo<'_> {
        ClientsRepo::new(self.conn())
    }

    fn trials(&mut self) -> TrialsRepo<'_> {
        TrialsRepo::new(self.conn())
    }

    fn tariffs(&mut self) -> TariffsRepo<'_> {
        TariffsRepo::new(self.conn())
    }

    fn promotions(&mut self) -> PromotionsRepo<'_> {
        PromotionsRepo::new(self.conn())
    }

    fn subscriptions(&mut self) -> SubscriptionsRepo<'_> {
        SubscriptionsRepo::new(self.conn())
    }

    fn orders(&mut self) -> OrdersRepo<'_> {
        OrdersRepo::new(self.conn())
    }

    fn payments(&mut self) -> PaymentsRepo<'_> {
        PaymentsRepo::new(self.conn())
    }

    fn ledger(&mut self) -> LedgerRepo<'_> {
        LedgerRepo::new(self.conn())
    }

    fn jobs(&mut self) -> JobsRepo<'_> {
        JobsRepo::new(self.conn())
    }

    fn events(&mut self) -> EventsRepo<'_> {
        EventsRepo::new(self.conn())
    }

    fn webhooks(&mut self) -> WebhooksRepo<'_> {
        WebhooksRepo::new(self.conn())
    }

    fn remna_events(&mut self) -> RemnaEventsRepo<'_> {
        RemnaEventsRepo::new(self.conn())
    }

    fn modules(&mut self) -> ModulesRepo<'_> {
        ModulesRepo::new(self.conn())
    }

    fn audit(&mut self) -> AuditRepo<'_> {
        AuditRepo::new(self.conn())
    }

    fn audit_journal(&mut self) -> AuditJournalRepo<'_> {
        AuditJournalRepo::new(self.conn())
    }

    fn tickets(&mut self) -> TicketsRepo<'_> {
        TicketsRepo::new(self.conn())
    }

    fn ticket_files(&mut self) -> TicketFilesRepo<'_> {
        TicketFilesRepo::new(self.conn())
    }

    fn saved_methods(&mut self) -> SavedMethodsRepo<'_> {
        SavedMethodsRepo::new(self.conn())
    }

    fn refunds(&mut self) -> RefundsRepo<'_> {
        RefundsRepo::new(self.conn())
    }

    fn payment_notices(&mut self) -> PaymentNoticesRepo<'_> {
        PaymentNoticesRepo::new(self.conn())
    }

    fn autorenew(&mut self) -> AutoRenewRepo<'_> {
        AutoRenewRepo::new(self.conn())
    }

    fn bot_state(&mut self) -> BotStateRepo<'_> {
        BotStateRepo::new(self.conn())
    }

    fn client_photos(&mut self) -> ClientPhotosRepo<'_> {
        ClientPhotosRepo::new(self.conn())
    }

    fn bot_images(&mut self) -> BotImagesRepo<'_> {
        BotImagesRepo::new(self.conn())
    }

    fn document_pages(&mut self) -> DocumentPagesRepo<'_> {
        DocumentPagesRepo::new(self.conn())
    }
}

impl Db for Conn {
    fn conn(&mut self) -> &mut PgConnection {
        &mut self.0
    }
}

impl Db for Tx {
    fn conn(&mut self) -> &mut PgConnection {
        &mut self.0
    }
}
