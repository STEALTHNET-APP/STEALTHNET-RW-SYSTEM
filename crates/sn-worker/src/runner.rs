use crate::handlers::{CoreHandlers, JobHandler, JobResult};
use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use sn_domain::jobs::{JobKind, JobPayload};
use sn_domain::order::OrderStatus;
use sn_services::checkout::cancel_if_unpaid;
use sn_services::fulfillment::abandoned_reason;
use sn_services::session_sweep::{START_DELAY, SessionSweep};
use sn_services::{Actor, ServiceError, Services};
use sn_store::{Db, JobRecord, LockSpace, audit_entity};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{Instant, MissedTickBehavior, interval, interval_at};
use tokio_util::sync::CancellationToken;

const REDELIVER_GRACE_MINUTES: i64 = 2;
const REDELIVER_QUIET_MINUTES: i64 = 30;
const REDELIVER_BATCH: i64 = 100;
const REFUND_POLL_BATCH: i64 = 100;
const MIN_HEARTBEAT: Duration = Duration::from_millis(50);

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub poll_interval: Duration,
    pub batch: i64,
    pub lease: Duration,
    pub events_interval: Duration,
    pub housekeeping_interval: Duration,
    pub autorenew_interval: Duration,
    pub reconcile_interval: Duration,
    pub module_tasks_interval: Duration,
    pub redeliver_interval: Duration,
    pub end_expired_interval: Duration,
    pub chain_watch_interval: Duration,
    pub node_watch_interval: Duration,
    pub renewals_interval: Duration,
    pub session_sweep_delay: Duration,
    pub reconcile_batch: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            poll_interval: Duration::from_secs(1),
            batch: 16,
            lease: Duration::from_secs(300),
            events_interval: Duration::from_secs(2),
            housekeeping_interval: Duration::from_secs(60),
            autorenew_interval: Duration::from_secs(60),
            reconcile_interval: Duration::from_secs(600),
            module_tasks_interval: Duration::from_secs(15),
            redeliver_interval: Duration::from_secs(300),
            end_expired_interval: Duration::from_secs(3_600),
            chain_watch_interval: Duration::from_secs(20),
            node_watch_interval: Duration::from_secs(15),
            renewals_interval: Duration::from_secs(300),
            session_sweep_delay: START_DELAY,
            reconcile_batch: 500,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Periodic {
    Housekeeping,
    AutoRenew,
    Reconcile,
    ModuleTasks,
    Redeliver,
    EndExpired,
    ChainWatch,
    NodeWatch,
    Renewals,
}

impl Periodic {
    pub const ALL: [Periodic; 9] = [
        Periodic::Housekeeping,
        Periodic::AutoRenew,
        Periodic::Reconcile,
        Periodic::ModuleTasks,
        Periodic::Redeliver,
        Periodic::EndExpired,
        Periodic::ChainWatch,
        Periodic::NodeWatch,
        Periodic::Renewals,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Periodic::Housekeeping => "housekeeping",
            Periodic::AutoRenew => "autorenew",
            Periodic::Reconcile => "reconcile",
            Periodic::ModuleTasks => "module_tasks",
            Periodic::Redeliver => "redeliver",
            Periodic::EndExpired => "end_expired",
            Periodic::ChainWatch => "chain_watch",
            Periodic::NodeWatch => "node_watch",
            Periodic::Renewals => "renewals",
        }
    }

    pub fn state_key(self) -> String {
        format!("worker.{}", self.name())
    }

    const fn lock_id(self) -> i64 {
        match self {
            Periodic::Housekeeping => 1,
            Periodic::AutoRenew => 2,
            Periodic::Reconcile => 3,
            Periodic::ModuleTasks => 4,
            Periodic::Redeliver => 5,
            Periodic::EndExpired => 6,
            Periodic::ChainWatch => 7,
            Periodic::NodeWatch => 8,
            Periodic::Renewals => 9,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeriodicState {
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_report: Option<serde_json::Value>,
}

fn time_field(value: &serde_json::Value, key: &str) -> Option<DateTime<Utc>> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

impl PeriodicState {
    pub fn from_json(value: &serde_json::Value) -> Self {
        PeriodicState {
            last_started_at: time_field(value, "last_started_at"),
            last_success_at: time_field(value, "last_success_at"),
            last_error: value
                .get("last_error")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            last_error_at: time_field(value, "last_error_at"),
            last_report: value.get("last_report").filter(|v| !v.is_null()).cloned(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "last_started_at": self.last_started_at.map(|t| t.to_rfc3339()),
            "last_success_at": self.last_success_at.map(|t| t.to_rfc3339()),
            "last_error": self.last_error,
            "last_error_at": self.last_error_at.map(|t| t.to_rfc3339()),
            "last_report": self.last_report,
        })
    }
}

fn json_error(err: serde_json::Error) -> ServiceError {
    ServiceError::Internal(err.to_string())
}

fn step(name: &str, result: Result<serde_json::Value, ServiceError>) -> serde_json::Value {
    match result {
        Ok(report) => report,
        Err(err) => {
            tracing::warn!(step = name, error = %err, "шаг обслуживания не выполнен");
            serde_json::json!({ "error": err.to_string() })
        }
    }
}

fn within(passed: chrono::Duration, limit: chrono::Duration) -> bool {
    passed >= chrono::Duration::zero() && passed < limit
}

fn rests(
    now: DateTime<Utc>,
    started: DateTime<Utc>,
    age: chrono::Duration,
    pause: chrono::Duration,
) -> bool {
    within(if started > now { age } else { now - started }, pause)
}

fn slot_taken(slot: i64, last: i64, age: chrono::Duration, every: i64) -> bool {
    let every = chrono::Duration::try_seconds(every).unwrap_or(chrono::Duration::MAX);
    last == slot || (last > slot && within(age, every))
}

async fn read_state(db: &mut impl Db, key: &str) -> Result<PeriodicState, ServiceError> {
    Ok(db
        .bot_state()
        .get(key)
        .await?
        .map(|value| PeriodicState::from_json(&value))
        .unwrap_or_default())
}

pub async fn periodic_state(
    services: &Services,
    periodic: Periodic,
) -> Result<PeriodicState, ServiceError> {
    let mut conn = services.store().conn().await?;
    read_state(&mut conn, &periodic.state_key()).await
}

pub struct Worker {
    services: Services,
    config: WorkerConfig,
    handler: Arc<dyn JobHandler>,
    by_kind: HashMap<JobKind, Arc<dyn JobHandler>>,
    sessions: Mutex<SessionSweep>,
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "без описания".to_string())
        .chars()
        .take(300)
        .collect()
}

impl Worker {
    pub fn new(services: Services, config: WorkerConfig) -> Self {
        let sessions = Mutex::new(SessionSweep::starting_in(config.session_sweep_delay));
        Worker {
            services,
            config,
            handler: Arc::new(CoreHandlers),
            by_kind: HashMap::new(),
            sessions,
        }
    }

    pub fn with_handler(mut self, handler: Arc<dyn JobHandler>) -> Self {
        self.handler = handler;
        self
    }

    pub fn with_kind_handler(mut self, kind: JobKind, handler: Arc<dyn JobHandler>) -> Self {
        self.by_kind.insert(kind, handler);
        self
    }

    fn handler_for(&self, kind: JobKind) -> &Arc<dyn JobHandler> {
        self.by_kind.get(&kind).unwrap_or(&self.handler)
    }

    async fn give_up(
        &self,
        job: &JobRecord,
        failed: bool,
        error: &str,
    ) -> Result<bool, ServiceError> {
        let mut tx = self.services.store().begin().await?;
        let recorded = if failed {
            tx.jobs().fail_leased(job, error).await?
        } else {
            tx.jobs().review_leased(job, error).await?
        };
        if recorded && let Ok(JobPayload::FulfillOrder { order_id }) = job.typed() {
            let reason = abandoned_reason(job.attempts, error);
            if tx.orders().abandon_plan(order_id, &reason).await? {
                tracing::warn!(job = %job.id, order = %order_id, "план заказа на ручной проверке больше не применяется: нужно решение администратора");
            }
        }
        tx.commit().await?;
        Ok(recorded)
    }

    async fn finish(&self, job: &JobRecord, result: JobResult) -> Result<(), ServiceError> {
        let recorded = match result {
            JobResult::Done => {
                let mut conn = self.services.store().conn().await?;
                conn.jobs().complete_leased(job).await?
            }
            JobResult::Review(reason) => {
                tracing::warn!(job = %job.id, kind = %job.kind, %reason, "задача ждёт ручной проверки");
                self.give_up(job, false, &reason).await?
            }
            JobResult::Fail(reason) => {
                tracing::error!(job = %job.id, kind = %job.kind, %reason, "задача провалена");
                self.give_up(job, true, &reason).await?
            }
            JobResult::Retry { error, .. } if job.exhausted() => {
                tracing::error!(job = %job.id, kind = %job.kind, %error, "попытки кончились");
                self.give_up(job, false, &error).await?
            }
            JobResult::Retry { after, error } => {
                let mut conn = self.services.store().conn().await?;
                conn.jobs()
                    .retry_leased(job, &error, self.services.now() + after)
                    .await?
            }
            JobResult::Wait { after, error } => {
                let mut conn = self.services.store().conn().await?;
                conn.jobs()
                    .postpone_leased(job, &error, self.services.now() + after)
                    .await?
            }
        };
        if !recorded {
            tracing::warn!(job = %job.id, kind = %job.kind, "итог задачи не записан: аренду уже перехватил другой обработчик");
        }
        Ok(())
    }

    async fn prolong(&self, job: &JobRecord) {
        let extended = match self.services.store().conn().await {
            Ok(mut conn) => conn.jobs().extend_lease(job, self.config.lease).await,
            Err(err) => Err(err),
        };
        match extended {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(job = %job.id, kind = %job.kind, "аренда задачи потеряна: её взял другой обработчик");
            }
            Err(err) => {
                tracing::warn!(job = %job.id, kind = %job.kind, error = %err, "аренда задачи не продлена");
            }
        }
    }

    async fn execute(&self, job: &JobRecord) -> JobResult {
        let payload = match job.typed() {
            Ok(payload) => payload,
            Err(err) => return JobResult::Fail(format!("задача не разобрана: {err}")),
        };
        let mut work = std::pin::pin!(
            std::panic::AssertUnwindSafe(self.handler_for(payload.kind()).handle(
                &self.services,
                &payload,
                job.attempts
            ))
            .catch_unwind()
        );
        let every = (self.config.lease / 3).max(MIN_HEARTBEAT);
        let mut heartbeat = interval_at(Instant::now() + every, every);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = &mut work => return match result {
                    Ok(result) => result,
                    Err(panic) => {
                        let text = panic_text(panic.as_ref());
                        tracing::error!(job = %job.id, kind = %job.kind, panic = %text, "задача упала с паникой, воркер продолжает работу");
                        JobResult::Fail(format!("задача упала: {text}"))
                    }
                },
                _ = heartbeat.tick() => self.prolong(job).await,
            }
        }
    }

    pub async fn tick(&self) -> Result<usize, ServiceError> {
        self.tick_lane(None).await
    }

    async fn tick_lane(&self, modules: Option<bool>) -> Result<usize, ServiceError> {
        let limit = usize::try_from(self.config.batch.max(1)).unwrap_or(1);
        let mut processed = 0usize;
        while processed < limit {
            let job = {
                let mut conn = self.services.store().conn().await?;
                conn.jobs()
                    .claim_lane(1, self.config.lease, modules)
                    .await?
                    .pop()
            };
            let Some(job) = job else {
                break;
            };
            let result = self.execute(&job).await;
            self.finish(&job, result).await?;
            processed += 1;
        }
        Ok(processed)
    }

    pub fn interval_of(&self, periodic: Periodic) -> Duration {
        match periodic {
            Periodic::Housekeeping => self.config.housekeeping_interval,
            Periodic::AutoRenew => self.config.autorenew_interval,
            Periodic::Reconcile => self.config.reconcile_interval,
            Periodic::ModuleTasks => self.config.module_tasks_interval,
            Periodic::Redeliver => self.config.redeliver_interval,
            Periodic::EndExpired => self.config.end_expired_interval,
            Periodic::ChainWatch => self.config.chain_watch_interval,
            Periodic::NodeWatch => self.config.node_watch_interval,
            Periodic::Renewals => self.config.renewals_interval,
        }
    }

    async fn claim(&self, periodic: Periodic) -> Result<bool, ServiceError> {
        let now = self.services.now();
        let every = chrono::Duration::from_std(self.interval_of(periodic))
            .unwrap_or_else(|_| chrono::Duration::seconds(60));
        let mut tx = self.services.store().begin().await?;
        if !tx.try_lock(LockSpace::Worker, periodic.lock_id()).await? {
            tx.rollback().await?;
            return Ok(false);
        }
        let key = periodic.state_key();
        let (mut state, age) = match tx.bot_state().get_aged(&key).await? {
            Some((value, age)) => (PeriodicState::from_json(&value), age),
            None => (PeriodicState::default(), chrono::Duration::zero()),
        };
        if let Some(started) = state.last_started_at {
            if rests(now, started, age, every * 9 / 10) {
                tx.rollback().await?;
                return Ok(false);
            }
            if started > now {
                tracing::warn!(
                    task = periodic.name(),
                    ahead_seconds = (started - now).num_seconds(),
                    "прошлый запуск записан впереди часов панели: часы переводили или у копий панели они разные, запускаем по часам базы"
                );
            }
        }
        state.last_started_at = Some(now);
        tx.bot_state().set(&key, &state.to_json()).await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn record(
        &self,
        periodic: Periodic,
        result: &Result<serde_json::Value, ServiceError>,
    ) -> Result<(), ServiceError> {
        let now = self.services.now();
        let key = periodic.state_key();
        let mut conn = self.services.store().conn().await?;
        let mut state = read_state(&mut conn, &key).await?;
        match result {
            Ok(report) => {
                state.last_success_at = Some(now);
                state.last_report = Some(report.clone());
            }
            Err(err) => {
                state.last_error = Some(err.to_string());
                state.last_error_at = Some(now);
            }
        }
        conn.bot_state().set(&key, &state.to_json()).await?;
        Ok(())
    }

    pub async fn run_periodic(
        &self,
        periodic: Periodic,
    ) -> Result<Option<serde_json::Value>, ServiceError> {
        if !self.claim(periodic).await? {
            return Ok(None);
        }
        let result = match periodic {
            Periodic::Housekeeping => self.housekeeping().await,
            Periodic::AutoRenew => self
                .services
                .autorenew()
                .run_due(self.services.now())
                .await
                .and_then(|report| serde_json::to_value(report).map_err(json_error)),
            Periodic::Reconcile => self
                .services
                .remna_sync()
                .reconcile(self.config.reconcile_batch)
                .await
                .and_then(|report| serde_json::to_value(report).map_err(json_error)),
            Periodic::ModuleTasks => self.schedule_module_tasks().await,
            Periodic::Redeliver => self.redeliver().await,
            Periodic::EndExpired => self
                .services
                .autorenew()
                .end_expired(self.services.now())
                .await
                .and_then(|report| serde_json::to_value(report).map_err(json_error)),
            Periodic::ChainWatch => self
                .services
                .inbox()
                .watch_chain(self.services.now())
                .await
                .and_then(|report| serde_json::to_value(report).map_err(json_error)),
            Periodic::NodeWatch => self
                .services
                .node_watch()
                .poll()
                .await
                .and_then(|report| serde_json::to_value(report).map_err(json_error)),
            Periodic::Renewals => self
                .services
                .renewals()
                .sweep(self.services.now())
                .await
                .and_then(|report| serde_json::to_value(report).map_err(json_error)),
        };
        self.record(periodic, &result).await?;
        result.map(Some)
    }

    async fn housekeeping(&self) -> Result<serde_json::Value, ServiceError> {
        let expired = {
            let mut conn = self.services.store().conn().await?;
            conn.payments()
                .expire_stale(self.services.now(), 500)
                .await?
        };
        let mut canceled = 0usize;
        let ctx = self.services.module_context();
        for payment_id in &expired {
            let mut tx = self.services.store().begin().await?;
            let payment = tx.payments().get(*payment_id).await?;
            if cancel_if_unpaid(self.services.modules(), &ctx, &mut tx, payment.order_id).await? {
                canceled += 1;
            }
            tx.commit().await?;
        }
        if !expired.is_empty() {
            tracing::info!(
                payments = expired.len(),
                orders = canceled,
                "просроченные счета отменены"
            );
        }
        let inbox = self.services.inbox();
        let purged = step(
            "purge_webhooks",
            inbox.purge_webhooks().await.map(serde_json::Value::from),
        );
        let sessions = step(
            "sweep_sessions",
            self.sessions
                .lock()
                .await
                .run(&self.services)
                .await
                .map(serde_json::Value::from),
        );
        let refunds = step(
            "poll_refunds",
            inbox
                .poll_refunds(REFUND_POLL_BATCH)
                .await
                .and_then(|report| serde_json::to_value(report).map_err(json_error)),
        );
        let (devices, traffic) = if self.services.remna().is_ok() {
            let subscriptions = self.services.subscriptions();
            let devices = step(
                "release_held_devices",
                subscriptions
                    .release_held_devices(self.services.now())
                    .await
                    .map(serde_json::Value::from),
            );
            let traffic = step(
                "settle_extra_traffic",
                subscriptions
                    .settle_extra_traffic(self.services.now())
                    .await
                    .map(serde_json::Value::from),
            );
            (devices, traffic)
        } else {
            (serde_json::Value::Null, serde_json::Value::Null)
        };
        Ok(serde_json::json!({
            "expired_payments": expired.len(),
            "canceled_orders": canceled,
            "purged_webhooks": purged,
            "removed_sessions": sessions,
            "refunds": refunds,
            "released_devices": devices,
            "settled_traffic": traffic,
        }))
    }

    async fn redeliver(&self) -> Result<serde_json::Value, ServiceError> {
        let now = self.services.now();
        let stalled = {
            let mut conn = self.services.store().conn().await?;
            conn.jobs()
                .stalled_fulfillments(
                    now - chrono::Duration::minutes(REDELIVER_GRACE_MINUTES),
                    now - chrono::Duration::minutes(REDELIVER_QUIET_MINUTES),
                    REDELIVER_BATCH,
                )
                .await?
        };
        let mut requeued = 0usize;
        for order_id in &stalled {
            let mut tx = self.services.store().begin().await?;
            let order = tx.orders().lock(*order_id).await?;
            if order.status != OrderStatus::Paid || tx.orders().refund_pending(order.id).await? {
                tx.rollback().await?;
                continue;
            }
            let queued = tx
                .jobs()
                .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
                .await?;
            if queued.armed() {
                tx.audit()
                    .record(&Actor::System.audit(
                        "order.redeliver",
                        audit_entity::ORDER,
                        Some(order.id.0),
                        serde_json::json!({
                            "job_id": queued.id(),
                            "state": queued.state(),
                            "reason": "оплаченный заказ остался без задачи выдачи",
                        }),
                    ))
                    .await?;
                requeued += 1;
                tracing::warn!(order = %order.id, job = %queued.id(), "оплаченный заказ без задачи выдачи поставлен в очередь снова");
            }
            tx.commit().await?;
        }
        Ok(serde_json::json!({
            "stalled": stalled.len(),
            "requeued": requeued,
        }))
    }

    async fn schedule_module_tasks(&self) -> Result<serde_json::Value, ServiceError> {
        let now = self.services.now();
        let mut scheduled = 0usize;
        let mut ahead = 0usize;
        let mut tx = self.services.store().begin().await?;
        let registry = self.services.modules();
        for id in sn_domain::module::ModuleId::ALL {
            let module = registry.get(id);
            let mut tasks = module.cleanup_tasks();
            if registry.is_enabled(id) {
                tasks.extend(module.tasks());
            }
            for task in tasks {
                let every = i64::try_from(task.every.as_secs())
                    .unwrap_or(i64::MAX)
                    .max(1);
                let slot = now.timestamp().div_euclid(every);
                let key = format!("worker.module_task.{}.{}", module.id().as_str(), task.name);
                let saved = tx.bot_state().get_aged(&key).await?;
                if let Some((last, age)) = saved.and_then(|(v, age)| Some((v.as_i64()?, age))) {
                    if slot_taken(slot, last, age, every) {
                        continue;
                    }
                    ahead += usize::from(last > slot);
                }
                tx.jobs()
                    .enqueue(
                        &JobPayload::ModuleTask {
                            module: module.id(),
                            task: task.name.to_string(),
                            payload: serde_json::json!({
                                "slot": slot,
                                "every_seconds": every,
                            }),
                        },
                        None,
                    )
                    .await?;
                tx.bot_state().set(&key, &serde_json::json!(slot)).await?;
                scheduled += 1;
            }
        }
        tx.commit().await?;
        if ahead > 0 {
            tracing::warn!(
                tasks = ahead,
                "прошлые запуски задач модулей записаны впереди часов панели: часы переводили или у копий панели они разные, ставим задачи по часам базы"
            );
        }
        Ok(serde_json::json!({ "scheduled": scheduled }))
    }

    async fn periodic(&self, periodic: Periodic) {
        let Some(_work) = self.services.pause_gate().try_enter() else {
            return;
        };
        match self.run_periodic(periodic).await {
            Ok(_) => {}
            Err(ServiceError::NotImplemented(what)) => {
                tracing::debug!(
                    task = periodic.name(),
                    what,
                    "периодическая работа ещё не реализована"
                );
            }
            Err(err) => {
                tracing::warn!(task = periodic.name(), error = %err, "периодическая работа не выполнена");
            }
        }
    }

    pub async fn run(self, cancel: CancellationToken) {
        let worker = Arc::new(self);
        let periodic = tokio::spawn({
            let worker = worker.clone();
            let cancel = cancel.clone();
            async move { worker.periodic_loop(&cancel).await }
        });
        let modules = tokio::spawn({
            let worker = worker.clone();
            let cancel = cancel.clone();
            async move { worker.module_loop(&cancel).await }
        });
        tracing::info!("фоновые задачи запущены");
        worker.queue_loop(&cancel).await;
        if let Err(err) = periodic.await {
            tracing::error!(error = %err, "периодические работы остановились с ошибкой");
        }
        if let Err(err) = modules.await {
            tracing::error!(error = %err, "задачи модулей остановились с ошибкой");
        }
        tracing::info!("фоновые задачи остановлены");
    }

    async fn module_loop(&self, cancel: &CancellationToken) {
        let mut jobs = interval(self.config.poll_interval);
        jobs.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = jobs.tick() => {
                    let Some(_work) = self.services.pause_gate().try_enter() else {
                        continue;
                    };
                    if let Err(err) = self.tick_lane(Some(true)).await {
                        tracing::warn!(error = %err, "очередь задач модулей недоступна");
                    }
                }
            }
        }
    }

    async fn queue_loop(&self, cancel: &CancellationToken) {
        let mut jobs = interval(self.config.poll_interval);
        let mut events = interval(self.config.events_interval);
        for timer in [&mut jobs, &mut events] {
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        }
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = jobs.tick() => {
                    let Some(_work) = self.services.pause_gate().try_enter() else {
                        continue;
                    };
                    if let Err(err) = self.tick_lane(Some(false)).await {
                        tracing::warn!(error = %err, "очередь задач недоступна");
                    }
                }
                _ = events.tick() => {
                    let Some(_work) = self.services.pause_gate().try_enter() else {
                        continue;
                    };
                    if let Err(err) = self.services.events().dispatch_pending(100).await {
                        tracing::warn!(error = %err, "события не разосланы модулям");
                    }
                }
            }
        }
    }

    async fn periodic_loop(&self, cancel: &CancellationToken) {
        let mut timers: Vec<(Periodic, tokio::time::Interval)> = Periodic::ALL
            .into_iter()
            .map(|periodic| {
                let mut timer = interval(self.interval_of(periodic));
                timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
                (periodic, timer)
            })
            .collect();
        loop {
            let due =
                futures_util::future::select_all(timers.iter_mut().map(|(periodic, timer)| {
                    let periodic = *periodic;
                    Box::pin(async move {
                        timer.tick().await;
                        periodic
                    })
                }));
            tokio::select! {
                _ = cancel.cancelled() => break,
                (periodic, _, _) = due => self.periodic(periodic).await,
            }
        }
    }
}
