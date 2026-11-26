use super::*;
use crate::Actor;
use crate::renewals::{self, CancelSource, Link, LinkStatus, title_of};
use sn_domain::ids::{PeriodId, SubscriptionId};
use sn_domain::order::{NewOrder, OrderChannel, PurchaseIntent};
use sn_domain::payment::{NewPayment, ProviderId, RENEWAL_METHOD};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::subscription::Subscription;
use sn_domain::tariff::Tariff;
use sn_payments::RenewalEvent;

pub(crate) const UNKNOWN_RENEWAL: &str = "продление подписки, которой нет в панели";

fn renewal_order(
    link: &Link,
    subscription: &Subscription,
    mut tariff: Tariff,
    amount: Money,
) -> ServiceResult<NewOrder> {
    let days = u32::try_from(link.days).unwrap_or(30).max(1);
    let wanted = link.period_id.map(PeriodId);
    tariff.on_sale = true;
    let index = tariff
        .periods
        .iter()
        .position(|p| Some(p.id) == wanted)
        .or_else(|| {
            (0..tariff.periods.len())
                .min_by_key(|i| (i64::from(tariff.periods[*i].days) - i64::from(days)).abs())
        })
        .ok_or_else(|| ServiceError::rejected("renewal.no_period", "у тарифа нет сроков"))?;
    {
        let period = &mut tariff.periods[index];
        period.days = days;
        period.active = true;
        period.prices.set(amount.currency, Some(amount.minor));
    }
    let period = tariff.periods[index].clone();
    let quote = quote(&QuoteRequest {
        currency: amount.currency,
        tariff: &tariff,
        period_id: period.id,
        extra_devices: 0,
        sell_extra_devices: false,
        adjustments: &[],
        promotion: None,
    })?;
    Ok(NewOrder {
        client_id: subscription.client_id,
        subscription_id: Some(subscription.id),
        kind: OrderKind::Renewal,
        intent: PurchaseIntent::Subscription {
            tariff_id: tariff.id,
            period_id: period.id,
            extra_devices: 0,
            target: Some(subscription.id),
        },
        quote,
        snapshot: Some(tariff.snapshot(&period)),
    })
}

impl PaymentInbox<'_> {
    async fn renewal_target(
        &self,
        tx: &mut Tx,
        link: &Link,
    ) -> ServiceResult<Option<(Subscription, Tariff)>> {
        let id = match link.subscription_id {
            Some(id) => Some(SubscriptionId(id)),
            None => {
                let first = tx.payments().get(PaymentId(link.first_payment_id)).await?;
                tx.orders().get(first.order_id).await?.subscription_id
            }
        };
        let Some(id) = id else {
            return Ok(None);
        };
        let subscription = match tx.subscriptions().get(id).await {
            Ok(subscription) => subscription,
            Err(StoreError::NotFound(_)) => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        if subscription.client_id.0 != link.client_id || subscription.ended_at.is_some() {
            return Ok(None);
        }
        let Some(tariff_id) = subscription.tariff_id else {
            return Ok(None);
        };
        if link.tariff_id.is_some_and(|t| t != tariff_id.0) {
            return Ok(None);
        }
        match tx.tariffs().get(tariff_id).await {
            Ok(tariff) => Ok(Some((subscription, tariff))),
            Err(StoreError::NotFound(_)) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_renewal(
        &self,
        tx: &mut Tx,
        provider: &str,
        link: &Link,
        order: NewOrder,
        amount: Money,
        renewal: &RenewalRef,
        charged_at: DateTime<Utc>,
    ) -> ServiceResult<AcceptResult> {
        let created = tx.orders().create(&order).await?;
        tx.orders()
            .set_channel(created.id, OrderChannel::Autorenew)
            .await?;
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: created.id,
                client_id: created.client_id,
                provider: ProviderId::from(provider),
                method: Some(RENEWAL_METHOD.into()),
                amount: amount.into(),
                expires_at: None,
            })
            .await?;
        let ctx = self.services.module_context();
        self.services
            .modules()
            .order_created(&ctx, tx, &created, &payment)
            .await?;
        renewals::record_charge(
            tx,
            link.id,
            payment.id,
            created.id,
            &renewal.txid,
            false,
            charged_at,
        )
        .await?;
        renewals::note_charge(
            tx,
            link,
            link.cycle(self.services),
            charged_at,
            self.services.now(),
        )
        .await?;
        self.settle_paid(tx, payment, Some(&renewal.txid), None)
            .await
    }

    pub(super) async fn accept_renewal(
        &self,
        provider: &str,
        renewal: &RenewalRef,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let amount = outcome.amount.and_then(Amount::as_money).ok_or_else(|| {
            ServiceError::rejected(
                "payment.amount_mismatch",
                "провайдер не подтвердил сумму продления",
            )
        })?;
        let now = self.services.now();
        let charged_at = renewal.charged_at.unwrap_or(now);
        let title = title_of(self.services, provider);
        let shown = format::money(amount);
        let mut tx = self.services.store().begin().await?;
        let Some(link) = renewals::lock_series(&mut tx, provider, &renewal.series).await? else {
            tx.payment_notices()
                .record(&NewPaymentNotice {
                    provider: provider.to_string(),
                    event_key: outcome.event_key.clone(),
                    kind: NoticeKind::ManualCredit,
                    payment_id: None,
                    client_id: None,
                    amount: Some(amount.into()),
                    text: clip(
                        &format!(
                            "{title} списала {shown} за продление подписки №{}, которой нет в панели. \
                             Найдите клиента по почте в кабинете {title} и зачислите деньги вручную",
                            renewal.series
                        ),
                        NOTICE_TEXT_LIMIT,
                    ),
                })
                .await?;
            tx.commit().await?;
            return Ok(AcceptResult::Ignored {
                reason: UNKNOWN_RENEWAL.into(),
            });
        };
        if let Some(existing) = tx
            .payments()
            .by_provider_txid(provider, &renewal.txid)
            .await?
        {
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: existing.id,
            });
        }
        let service = self.services.settings().currency;
        if link.amount().map(|a| a.currency) != Some(amount.currency)
            || amount.currency != service
            || amount.minor <= 0
        {
            renewals::notice(
                &mut tx,
                &link,
                format!("renewal_currency:{}", renewal.txid),
                NoticeKind::ManualCredit,
                Some(amount.into()),
                format!(
                    "{title} списала {shown} за продление, но валюта не совпала с валютой сервиса или подписки. \
                     Подписка не продлена: продлите её или зачислите деньги вручную"
                ),
            )
            .await?;
            tx.commit().await?;
            return Ok(AcceptResult::Ignored {
                reason: "валюта продления не совпала".into(),
            });
        }
        let last = renewals::last_charge(&mut tx, link.id).await?;
        if renewals::quick_repeat(last, charged_at) {
            renewals::notice(
                &mut tx,
                &link,
                format!("renewal_repeat:{}", renewal.txid),
                NoticeKind::ManualCredit,
                Some(amount.into()),
                format!(
                    "{title} прислала второе списание {shown} по одной подписке меньше чем через {} дней \
                     после прошлого (№{}). Подписка не продлена автоматически: сверьте списание в кабинете {title} \
                     и продлите подписку или зачислите деньги вручную",
                    renewals::QUICK_REPEAT_DAYS,
                    renewal.txid
                ),
            )
            .await?;
            tx.commit().await?;
            return Ok(AcceptResult::Ignored {
                reason: "второе списание за неделю ждёт проверки".into(),
            });
        }
        let after_off = !link.status().live();
        let result = match self.renewal_target(&mut tx, &link).await? {
            Some((subscription, tariff)) => {
                let order = renewal_order(&link, &subscription, tariff, amount)?;
                let result = self
                    .record_renewal(&mut tx, provider, &link, order, amount, renewal, charged_at)
                    .await?;
                if after_off && let AcceptResult::Accepted { payment_id, .. } = &result {
                    renewals::notify(
                        &mut tx,
                        link.client_id,
                        crate::autorenew::CHARGED_AFTER_OFF_TEMPLATE,
                        serde_json::json!({
                            "subscription_id": subscription.id,
                            "payment_id": payment_id,
                            "amount": shown,
                        }),
                    )
                    .await?;
                }
                result
            }
            None => {
                let order = buyer::topup_order(ClientId(link.client_id), amount)?;
                let result = self
                    .record_renewal(&mut tx, provider, &link, order, amount, renewal, charged_at)
                    .await?;
                renewals::notice(
                    &mut tx,
                    &link,
                    format!("renewal_balance:{}", renewal.txid),
                    NoticeKind::ManualCredit,
                    Some(amount.into()),
                    format!(
                        "{title} списала {shown} за продление, но подписки в панели уже нет или у неё другой тариф. \
                         Деньги зачислены на баланс клиента, автопродление в {title} отменяется"
                    ),
                )
                .await?;
                renewals::request_cancel(&mut tx, &link, CancelSource::Gone, Actor::System, now)
                    .await?;
                result
            }
        };
        tx.commit().await?;
        Ok(result)
    }

    pub(super) async fn renewal_after_refund(&self, result: &AcceptResult) {
        let payment = match result {
            AcceptResult::Accepted { payment_id, .. }
            | AcceptResult::AlreadyApplied { payment_id }
            | AcceptResult::StatusChanged { payment_id, .. } => *payment_id,
            AcceptResult::Ignored { .. } => return,
        };
        if let Err(err) = self.services.renewals().check_soon(payment).await {
            tracing::warn!(payment = %payment, error = %err, "сверка автопродления после возврата не назначена");
        }
    }

    pub(super) async fn accept_renewal_event(
        &self,
        provider: &str,
        event: &RenewalEvent,
    ) -> ServiceResult<AcceptResult> {
        let now = self.services.now();
        let (id, status, until) = {
            let mut conn = self.services.store().conn().await?;
            match event {
                RenewalEvent::Failed { series, .. } => (
                    renewals::series_of_contract(&mut conn, provider, series).await?,
                    LinkStatus::Failed,
                    None,
                ),
                RenewalEvent::Cancelled {
                    contract, until, ..
                } => (
                    renewals::series_of_contract(&mut conn, provider, contract).await?,
                    LinkStatus::Cancelled,
                    *until,
                ),
            }
        };
        let Some(id) = id else {
            return Ok(AcceptResult::Ignored {
                reason: "подписки платёжной системы нет в панели".into(),
            });
        };
        let mut tx = self.services.store().begin().await?;
        let Some(link) = renewals::lock_link(&mut tx, id).await? else {
            return Ok(AcceptResult::Ignored {
                reason: "подписки платёжной системы нет в панели".into(),
            });
        };
        let closed = renewals::close(
            &mut tx,
            &link,
            status,
            Some(CancelSource::Provider),
            Actor::System,
            until,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(AcceptResult::Ignored {
            reason: match (closed, status) {
                (true, LinkStatus::Failed) => "автопродление закрыто: списание не прошло".into(),
                (true, _) => "автопродление отменено".into(),
                (false, _) => "автопродление уже закрыто".into(),
            },
        })
    }
}
