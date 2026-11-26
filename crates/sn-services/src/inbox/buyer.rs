use super::*;
use crate::checkout::CartRequest;
use sn_domain::client::{Client, IdentityKind};
use sn_domain::ids::{PeriodId, SubscriptionId, TariffId};
use sn_domain::order::NewOrder;
use sn_domain::payment::ProviderId;
use sn_domain::pricing::{BonusTerms, TopUpLimits, topup_quote};
use sn_payments::{BuyerPurpose, PlanBinding};

pub const PAID_FROM_BALANCE: &str = "заказ оплачен с баланса после пополнения";

pub(super) fn topup_order(client_id: ClientId, amount: Money) -> ServiceResult<NewOrder> {
    Ok(NewOrder {
        client_id,
        subscription_id: None,
        kind: OrderKind::TopUp,
        intent: sn_domain::order::PurchaseIntent::TopUp { amount },
        quote: topup_quote(
            amount,
            TopUpLimits {
                min: amount,
                max: amount,
            },
        )?,
        snapshot: None,
    })
}

impl PaymentInbox<'_> {
    fn title_of(&self, provider: &str) -> String {
        self.services
            .payments()
            .get(provider)
            .map_or_else(|| provider.to_string(), |p| p.title().to_string())
    }

    pub(super) async fn accept_buyer(
        &self,
        provider: &str,
        buyer: &Buyer,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let (existing, client) = {
            let mut conn = self.services.store().conn().await?;
            let existing = conn
                .payments()
                .by_provider_txid(provider, &buyer.txid)
                .await?;
            let client = conn
                .clients()
                .find_by_identity(IdentityKind::Telegram, &buyer.telegram_id.to_string())
                .await?;
            (existing, client)
        };
        if let Some(existing) = existing {
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: existing.id,
            });
        }
        let amount = outcome.amount.and_then(Amount::as_money).ok_or_else(|| {
            ServiceError::rejected("payment.amount_mismatch", "провайдер не подтвердил сумму")
        })?;
        let title = self.title_of(provider);
        let shown = format::money(amount);
        let currency = self.services.settings().currency;
        if amount.currency != currency {
            self.waiting_credit(
                provider,
                outcome,
                None,
                amount,
                format!(
                    "Оплата {title} на {shown} от Telegram ID {} пришла в другой валюте, чем валюта \
                     панели ({}). Зачислите деньги вручную или верните покупателю",
                    buyer.telegram_id,
                    currency.code()
                ),
            )
            .await?;
            return Ok(AcceptResult::Ignored {
                reason: "валюта оплаты не совпала с валютой панели".into(),
            });
        }
        let Some(client) = client else {
            self.waiting_credit(
                provider,
                outcome,
                None,
                amount,
                format!(
                    "Оплата {title} на {shown} от Telegram ID {} не нашла клиента: в панели нет \
                     клиента с таким Telegram. Найдите клиента и зачислите деньги вручную",
                    buyer.telegram_id
                ),
            )
            .await?;
            return Ok(AcceptResult::Ignored {
                reason: "клиент с таким Telegram ID не найден".into(),
            });
        };
        match &buyer.purpose {
            BuyerPurpose::TopUp => {
                self.buyer_topup(provider, &client, amount, buyer, outcome)
                    .await
            }
            BuyerPurpose::Plan { .. } => {
                self.buyer_plan(provider, &client, amount, buyer, outcome)
                    .await
            }
        }
    }

    async fn waiting_credit(
        &self,
        provider: &str,
        outcome: &WebhookOutcome,
        client: Option<ClientId>,
        amount: Money,
        text: String,
    ) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let recorded = conn
            .payment_notices()
            .record(&NewPaymentNotice {
                provider: provider.to_string(),
                event_key: outcome.event_key.clone(),
                kind: NoticeKind::ManualCredit,
                payment_id: None,
                client_id: client,
                amount: Some(amount.into()),
                text: clip(&text, NOTICE_TEXT_LIMIT),
            })
            .await?;
        if recorded.is_some() {
            tracing::warn!(
                provider,
                event = %outcome.event_key,
                "оплата ждёт ручного зачисления"
            );
        }
        Ok(())
    }

    async fn buyer_topup(
        &self,
        provider: &str,
        client: &Client,
        amount: Money,
        buyer: &Buyer,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let now = self.services.now();
        let latest = {
            let mut conn = self.services.store().conn().await?;
            match conn
                .payments()
                .latest_waiting(client.id, provider, now)
                .await?
            {
                Some(payment) => {
                    let order = conn.orders().get(payment.order_id).await?;
                    Some((payment, order))
                }
                None => None,
            }
        };
        if let Some((payment, order)) = &latest
            && order.kind == OrderKind::TopUp
            && payment.amount == Amount::from(amount)
        {
            return self
                .accept_paid(provider, &PaymentRef::Payment(payment.id), outcome, None)
                .await;
        }
        let terms = if latest
            .as_ref()
            .is_none_or(|(_, order)| order.kind == OrderKind::TopUp)
        {
            self.services
                .payments()
                .topup_bonus(provider, None, amount.currency)
        } else {
            None
        };
        let result = self
            .record_external(
                provider,
                client.id,
                topup_order(client.id, amount)?,
                amount,
                &buyer.txid,
                terms,
            )
            .await?;
        if let Some((payment, order)) = latest.filter(|(_, order)| {
            order.kind != OrderKind::TopUp && order.status == OrderStatus::Pending
        }) {
            match self.pay_from_balance(&payment, &order).await {
                Ok(true) => {
                    tracing::info!(order = %order.id, "заказ оплачен с баланса после пополнения")
                }
                Ok(false) => {
                    tracing::info!(order = %order.id, "после пополнения на заказ не хватило баланса")
                }
                Err(err) => {
                    tracing::warn!(order = %order.id, error = %err, "заказ после пополнения не оплатился с баланса")
                }
            }
        }
        Ok(result)
    }

    async fn buyer_plan(
        &self,
        provider: &str,
        client: &Client,
        amount: Money,
        buyer: &Buyer,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let BuyerPurpose::Plan { plan, series } = &buyer.purpose else {
            return self
                .buyer_topup(provider, client, amount, buyer, outcome)
                .await;
        };
        let registry = self.services.payments();
        let binding = registry
            .get(provider)
            .and_then(|p| p.plan_binding(&registry.config(provider), plan.as_str()));
        let Some(binding) = binding else {
            return self
                .record_external(
                    provider,
                    client.id,
                    topup_order(client.id, amount)?,
                    amount,
                    &buyer.txid,
                    None,
                )
                .await;
        };
        let target = self
            .series_target(provider, client.id, series, binding)
            .await?;
        let cart = CartRequest {
            tariff_id: TariffId(binding.tariff_id),
            period_id: PeriodId(binding.period_id),
            extra_devices: 0,
            subscription_id: target,
            promo_code: None,
        };
        match self.services.checkout().plan_order(client.id, &cart).await {
            Ok(order) => {
                self.record_external(provider, client.id, order, amount, &buyer.txid, None)
                    .await
            }
            Err(err) => {
                tracing::warn!(client = %client.id, plan = %plan, error = %err, "подписка по оплате не выдана, деньги идут на баланс");
                let result = self
                    .record_external(
                        provider,
                        client.id,
                        topup_order(client.id, amount)?,
                        amount,
                        &buyer.txid,
                        None,
                    )
                    .await?;
                self.waiting_credit(
                    provider,
                    outcome,
                    Some(client.id),
                    amount,
                    format!(
                        "Подписку по оплате {} на {} выдать не удалось: {err}. Деньги зачислены \
                         на баланс клиента: выдайте подписку вручную или оставьте деньги на балансе",
                        self.title_of(provider),
                        format::money(amount)
                    ),
                )
                .await?;
                Ok(result)
            }
        }
    }

    async fn series_target(
        &self,
        provider: &str,
        client_id: ClientId,
        series: &str,
        binding: PlanBinding,
    ) -> ServiceResult<Option<SubscriptionId>> {
        let mut conn = self.services.store().conn().await?;
        let previous = conn.payments().latest_in_series(provider, series).await?;
        if let Some(previous) = previous.filter(|p| p.client_id == client_id) {
            let order = conn.orders().get(previous.order_id).await?;
            if let Some(id) = order.subscription_id
                && let Ok(subscription) = conn.subscriptions().get(id).await
                && subscription.client_id == client_id
                && subscription.ended_at.is_none()
            {
                return Ok(Some(id));
            }
        }
        Ok(conn
            .subscriptions()
            .by_client(client_id)
            .await?
            .into_iter()
            .filter(|s| s.ended_at.is_none() && s.tariff_id == Some(TariffId(binding.tariff_id)))
            .max_by_key(|s| s.id)
            .map(|s| s.id))
    }

    async fn record_external(
        &self,
        provider: &str,
        client_id: ClientId,
        order: NewOrder,
        amount: Money,
        txid: &str,
        bonus: Option<BonusTerms>,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let created = tx.orders().create(&order).await?;
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: created.id,
                client_id,
                provider: ProviderId::from(provider),
                method: None,
                amount: amount.into(),
                expires_at: None,
            })
            .await?;
        if let Some(terms) = bonus {
            tx.payments().promise_bonus(payment.id, &terms).await?;
        }
        let ctx = self.services.module_context();
        self.services
            .modules()
            .order_created(&ctx, &mut tx, &created, &payment)
            .await?;
        let result = self.settle_paid(&mut tx, payment, Some(txid), None).await?;
        tx.commit().await?;
        Ok(result)
    }

    async fn pay_from_balance(&self, waiting: &Payment, order: &Order) -> ServiceResult<bool> {
        let total = order.quote.total;
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().lock(order.id).await?;
        if order.status != OrderStatus::Pending {
            return Ok(false);
        }
        let balance = tx
            .ledger()
            .balance(order.client_id, Wallet::Main, total.currency)
            .await?;
        if balance.minor < total.minor {
            return Ok(false);
        }
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id: order.client_id,
                provider: ProviderId::from(providers::BALANCE),
                method: None,
                amount: total.into(),
                expires_at: None,
            })
            .await?;
        let delta = total.checked_neg()?;
        let appended = match tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: order.client_id,
                wallet: Wallet::Main,
                amount: delta,
                kind: LedgerKind::Purchase,
                order_id: Some(order.id),
                payment_id: Some(payment.id),
                admin_id: None,
                comment: None,
                idempotency_key: Some(format!("purchase:order:{}", order.id)),
            })
            .await
        {
            Ok(appended) => appended,
            Err(StoreError::InsufficientFunds) => return Ok(false),
            Err(err) => return Err(err.into()),
        };
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: order.client_id,
                    wallet: Wallet::Main,
                    kind: LedgerKind::Purchase,
                    delta,
                })
                .await?;
        }
        match tx
            .payments()
            .transition(
                waiting.id,
                PaymentStatus::Canceled,
                None,
                Some(PAID_FROM_BALANCE),
            )
            .await
        {
            Ok(_) | Err(StoreError::Invalid(_)) => {}
            Err(err) => return Err(err.into()),
        }
        self.settle_paid(&mut tx, payment, None, None).await?;
        tx.commit().await?;
        Ok(true)
    }
}
