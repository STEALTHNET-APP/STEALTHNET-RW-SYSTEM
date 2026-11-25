use super::*;
use crate::FixedClock;

const MUTE: &str = "mute";

struct Mute;

#[async_trait::async_trait]
impl PaymentProvider for Mute {
    fn id(&self) -> &'static str {
        MUTE
    }

    fn title(&self) -> &'static str {
        "Касса без запроса статуса возврата"
    }

    fn currencies(&self) -> &'static [Currency] {
        &Currency::ALL
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "СБП",
            hint: "Тестовая касса",
        }
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        Ok(Invoice {
            pay_url: format!("https://mute.example.com/{}", request.payment_public_id),
            external_id: Some(format!("m-{}", request.payment_id)),
            expires_at: None,
            payload: serde_json::json!({}),
        })
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        _request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::BadSignature)
    }

    async fn refund(
        &self,
        _config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("mr-{}", request.payment_id)),
            completed: false,
        })
    }
}

async fn polling_kit(kassa: Arc<Kassa>) -> Option<(Kit, Arc<FixedClock>)> {
    let clock = Arc::new(FixedClock::new(moment(chrono::Duration::zero())));
    let kit = kassa_kit_with(kassa, clock.clone(), vec![Arc::new(Mute)]).await?;
    Some((kit, clock))
}

fn advance(kit: &Kit, clock: &FixedClock, by: chrono::Duration) {
    clock.set(kit.services.now() + by);
}

async fn paid_topup(kit: &Kit, client_id: ClientId, minor: i64) -> Payment {
    let topped = kassa_topup(kit, client_id, minor).await;
    kit.services
        .inbox()
        .apply_outcome(KASSA, &outcome(&topped, ProviderStatus::Paid))
        .await
        .unwrap();
    topped
}

async fn request(kit: &Kit, admin: AdminId, payment: &Payment) {
    assert!(
        kit.services
            .inbox()
            .refund(
                payment.id,
                &refund_command(None, RefundSubscriptionAction::Keep, None),
                admin,
            )
            .await
            .unwrap()
            .pending
    );
}

fn refund_state(refund: &str, status: ProviderStatus, minor: i64) -> Option<WebhookOutcome> {
    Some(WebhookOutcome {
        event_key: format!("refund:{refund}"),
        payment: None,
        provider_txid: None,
        status,
        amount: Some(Amount::new(minor, PayCurrency::Usd)),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

fn lookups(kassa: &Kassa) -> Vec<String> {
    kassa.lookups.lock().unwrap().clone()
}

#[tokio::test]
async fn сто_неопрашиваемых_заявок_не_мешают_подтвердить_и_отменить_возврат_кассы() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = polling_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7700).await;
    let tariff = standard(&kit.services).await;
    let mute = pending(
        &kit,
        client_id,
        &tariff,
        MUTE,
        Amount::new(699, PayCurrency::Usd),
        None,
    )
    .await;
    let inbox = kit.services.inbox();
    inbox
        .apply_outcome(MUTE, &outcome(&mute, ProviderStatus::Paid))
        .await
        .unwrap();
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        for n in 0..100 {
            conn.payments()
                .create_provider_refund(&NewProviderRefund {
                    bonus_minor: 0,
                    payment_id: mute.id,
                    amount: Amount::new(1, PayCurrency::Usd),
                    subscription_action: RefundSubscriptionAction::Keep,
                    reason: None,
                    admin_id: Some(admin),
                    provider_refund_id: Some(format!("mr-{n}")),
                    hold_entry_id: None,
                    requested_at: kit.services.now() - chrono::Duration::hours(2),
                })
                .await
                .unwrap();
        }
    }
    let first = paid_topup(&kit, client_id, 1_000).await;
    request(&kit, admin, &first).await;
    advance(&kit, &clock, chrono::Duration::minutes(2));
    *kassa.refund_status.lock().unwrap() = refund_state("kr-1", ProviderStatus::Refunded, 1_000);
    assert_eq!(
        inbox.poll_refunds(100).await.unwrap(),
        RefundPollReport {
            settled: 1,
            ..RefundPollReport::default()
        }
    );
    assert_eq!(
        payment(&kit, first.id).await.status,
        PaymentStatus::Refunded
    );
    let second = paid_topup(&kit, client_id, 400).await;
    request(&kit, admin, &second).await;
    assert_eq!(balance(&kit.services, client_id).await, 0);
    advance(&kit, &clock, chrono::Duration::minutes(2));
    *kassa.refund_status.lock().unwrap() =
        refund_state("kr-2", ProviderStatus::RefundDeclined, 400);
    assert_eq!(
        inbox.poll_refunds(100).await.unwrap(),
        RefundPollReport {
            settled: 1,
            ..RefundPollReport::default()
        }
    );
    assert_eq!(
        provider_refunds(&kit, second.id).await[0].status,
        ProviderRefundStatus::Declined
    );
    assert_eq!(balance(&kit.services, client_id).await, 400);
    assert_eq!(
        lookups(&kassa),
        vec!["refund:kr-1".to_string(), "refund:kr-2".to_string()]
    );
    let untouched = provider_refunds(&kit, mute.id).await;
    assert_eq!(untouched.len(), 100);
    assert!(
        untouched
            .iter()
            .all(|r| r.status == ProviderRefundStatus::Pending && r.polled_at.is_none())
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пачка_опроса_возвратов_ротируется_по_времени_последнего_опроса() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = polling_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7701).await;
    let mut payments = Vec::new();
    for minor in [300, 400, 500] {
        let topped = paid_topup(&kit, client_id, minor).await;
        request(&kit, admin, &topped).await;
        payments.push(topped);
        advance(&kit, &clock, chrono::Duration::seconds(1));
    }
    *kassa.refund_status.lock().unwrap() = refund_state("kr-1", ProviderStatus::Pending, 300);
    let inbox = kit.services.inbox();
    assert_eq!(
        inbox.poll_refunds(2).await.unwrap(),
        RefundPollReport::default()
    );
    assert!(lookups(&kassa).is_empty());
    advance(&kit, &clock, chrono::Duration::minutes(2));
    let waiting = |count: usize| RefundPollReport {
        waiting: count,
        ..RefundPollReport::default()
    };
    assert_eq!(inbox.poll_refunds(2).await.unwrap(), waiting(2));
    assert_eq!(
        lookups(&kassa),
        vec!["refund:kr-1".to_string(), "refund:kr-2".to_string()]
    );
    assert_eq!(inbox.poll_refunds(2).await.unwrap(), waiting(1));
    assert_eq!(lookups(&kassa)[2..], ["refund:kr-3".to_string()]);
    assert_eq!(inbox.poll_refunds(2).await.unwrap(), waiting(0));
    assert_eq!(lookups(&kassa).len(), 3);
    advance(&kit, &clock, chrono::Duration::seconds(61));
    assert_eq!(inbox.poll_refunds(2).await.unwrap(), waiting(2));
    assert_eq!(
        lookups(&kassa)[3..],
        ["refund:kr-1".to_string(), "refund:kr-2".to_string()]
    );
    assert_eq!(inbox.poll_refunds(2).await.unwrap(), waiting(1));
    assert_eq!(lookups(&kassa)[5..], ["refund:kr-3".to_string()]);
    for topped in &payments {
        let requests = provider_refunds(&kit, topped.id).await;
        assert_eq!(requests[0].status, ProviderRefundStatus::Pending);
        assert_eq!(requests[0].polled_at, Some(kit.services.now()));
    }
    kit.db.cleanup().await;
}

#[tokio::test]
async fn заявка_без_номера_возврата_в_опрос_не_попадает() {
    let kassa = Arc::new(Kassa::default());
    let Some((kit, clock)) = polling_kit(kassa.clone()).await else {
        return;
    };
    let admin = admin(&kit).await;
    let client_id = client(&kit.services, 7702).await;
    let topped = paid_topup(&kit, client_id, 500).await;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.payments()
            .create_provider_refund(&NewProviderRefund {
                bonus_minor: 0,
                payment_id: topped.id,
                amount: Amount::new(500, PayCurrency::Usd),
                subscription_action: RefundSubscriptionAction::Keep,
                reason: None,
                admin_id: Some(admin),
                provider_refund_id: None,
                hold_entry_id: None,
                requested_at: kit.services.now(),
            })
            .await
            .unwrap();
    }
    advance(&kit, &clock, chrono::Duration::minutes(5));
    assert_eq!(
        kit.services.inbox().poll_refunds(10).await.unwrap(),
        RefundPollReport::default()
    );
    assert!(lookups(&kassa).is_empty());
    assert_eq!(provider_refunds(&kit, topped.id).await[0].polled_at, None);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn касса_без_поиска_по_своему_номеру_опрашивается_по_номеру_панели() {
    let kassa = Arc::new(Kassa::default());
    *kassa.knows_order_only.lock().unwrap() = true;
    let Some((kit, _)) = polling_kit(kassa.clone()).await else {
        return;
    };
    let client_id = client(&kit.services, 7707).await;
    let topped = kassa_topup(&kit, client_id, 1_000).await;
    let txid = topped.provider_txid.clone().unwrap();
    let inbox = kit.services.inbox();
    *kassa.status.lock().unwrap() = Some(outcome(&topped, ProviderStatus::Pending));
    assert_eq!(
        inbox.poll_payment(topped.id).await.unwrap(),
        PollResult::Pending
    );
    *kassa.status.lock().unwrap() = Some(outcome(&topped, ProviderStatus::Paid));
    assert_eq!(
        inbox.poll_payment(topped.id).await.unwrap(),
        PollResult::Settled
    );
    assert_eq!(
        lookups(&kassa),
        vec![topped.public_id.to_string(), topped.public_id.to_string()]
    );
    let mut conn = kit.services.store().conn().await.unwrap();
    let paid = conn.payments().get(topped.id).await.unwrap();
    assert_eq!(paid.status, PaymentStatus::Paid);
    assert_eq!(paid.provider_txid.as_deref(), Some(txid.as_str()));
    drop(conn);
    assert_eq!(balance(&kit.services, client_id).await, 1_000);
    kit.db.cleanup().await;
}
