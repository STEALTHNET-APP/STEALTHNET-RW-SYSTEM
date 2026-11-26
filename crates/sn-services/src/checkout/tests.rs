use super::testkit::*;
use super::*;
use crate::fulfillment::FulfillmentOutcome;
use async_trait::async_trait;
use sn_domain::money::Currency;
use sn_domain::pricing::QuoteLine;
use sn_domain::pricing::QuoteLineKind;
use sn_domain::subscription::TRIAL_TAG;
use sn_domain::traffic::TrafficPolicy;
use sn_modules::{Module, ModuleContext, ModuleError};
use std::sync::Mutex;

fn request(cart: CartRequest, provider: &str) -> CheckoutRequest {
    CheckoutRequest {
        cart,
        provider: provider.into(),
        method: None,
        channel: Channel::Cabinet,
        return_url: Some(format!("{PUBLIC_URL}/cabinet/pay")),
    }
}

#[tokio::test]
async fn счёт_выставляется_после_заказа_и_платежа() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 5_093_118_422).await;
    let tariff = standard(&kit.services).await;
    let result = kit
        .services
        .checkout()
        .start(client_id, &request(cart(&tariff, 90, None), TEST_PAY))
        .await
        .unwrap();
    let CheckoutResult::Redirect {
        order_id,
        payment_id,
        pay_url,
    } = result
    else {
        panic!("ожидалась ссылка на оплату");
    };
    assert_eq!(pay_url, format!("{PUBLIC_URL}/api/pay/{payment_id}"));
    let mut conn = kit.services.store().conn().await.unwrap();
    let payment = conn.payments().by_public_id(payment_id).await.unwrap();
    assert_eq!(payment.status, PaymentStatus::Pending);
    assert_eq!(payment.amount, Amount::new(699, PayCurrency::Usd));
    assert_eq!(payment.provider_txid, Some(format!("tp-{}", payment.id)));
    assert_eq!(
        payment.pay_url,
        Some(format!("https://pay.example.com/{payment_id}"))
    );
    let expires = payment.expires_at.unwrap() - kit.services.now();
    assert!(expires > Duration::minutes(55) && expires <= Duration::minutes(60));
    let order = conn.orders().by_public_id(order_id).await.unwrap();
    assert_eq!(order.status, OrderStatus::Pending);
    assert_eq!(order.kind, OrderKind::New);
    assert_eq!(order.snapshot.as_ref().unwrap().days, 90);
    drop(conn);
    let invoices = kit.pay.invoices.lock().unwrap().clone();
    assert_eq!(invoices.len(), 1);
    assert_eq!(
        invoices[0].webhook_url,
        format!("{PUBLIC_URL}/api/webhooks/pay/{TEST_PAY}")
    );
    assert_eq!(invoices[0].client.telegram_id, Some(5_093_118_422));
    assert_eq!(
        invoices[0].return_url.as_deref(),
        Some("https://panel.example.com/cabinet/pay")
    );
    assert!(invoices[0].save_payment_method);
    assert_eq!(
        jobs(&kit.services).await,
        Vec::<JobPayload>::new(),
        "опрос счёта ставится на потом"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ошибка_кассы_отменяет_платёж_и_заказ() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 77).await;
    let tariff = standard(&kit.services).await;
    *kit.pay.fail_invoice.lock().unwrap() = true;
    let err = kit
        .services
        .checkout()
        .start(client_id, &request(cart(&tariff, 30, None), TEST_PAY))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Payment(_)), "{err:?}");
    let mut conn = kit.services.store().conn().await.unwrap();
    let orders = conn
        .orders()
        .for_client(client_id, Default::default())
        .await
        .unwrap();
    assert_eq!(orders.items.len(), 1);
    assert_eq!(orders.items[0].status, OrderStatus::Canceled);
    let payments = conn.payments().for_order(orders.items[0].id).await.unwrap();
    assert_eq!(payments[0].status, PaymentStatus::Failed);
    drop(conn);
    let listed = kit
        .services
        .admin()
        .payments_view(
            &crate::admin::PaymentFilter {
                client_id: Some(client_id),
                ..Default::default()
            },
            sn_domain::paging::PageRequest::new(None, None),
        )
        .await
        .unwrap();
    assert_eq!(
        listed.items[0].failure_reason.as_deref(),
        Some("Счёт не выставлен: касса недоступна"),
        "оператор видит, почему платёж не прошёл"
    );
    let history = kit
        .services
        .clients()
        .history(
            client_id,
            sn_domain::paging::PageRequest::new(None, None),
            true,
        )
        .await
        .unwrap();
    assert!(
        history.items.iter().any(|item| matches!(
            &item.record,
            crate::clients::HistoryRecord::Payment { failure_reason: Some(reason), .. }
                if reason == "Счёт не выставлен: касса недоступна"
        )),
        "причина видна и в карточке клиента"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn недоступный_способ_оплаты_отклоняется() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 78).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 420).await;
    let unknown = kit
        .services
        .checkout()
        .start(client_id, &request(cart(&tariff, 90, None), "heleket"))
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), "checkout.method_unavailable");
    let short = kit
        .services
        .checkout()
        .start(
            client_id,
            &request(cart(&tariff, 90, None), providers::BALANCE),
        )
        .await
        .unwrap_err();
    assert_eq!(short.code(), "checkout.method_unavailable");
    assert!(short.to_string().contains("Не хватает: на балансе $4.20"));
    assert_eq!(balance(&kit.services, client_id).await, 420);
    let mut conn = kit.services.store().conn().await.unwrap();
    assert!(
        conn.orders()
            .for_client(client_id, Default::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    drop(conn);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оплата_с_баланса_одной_транзакцией() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 79).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 90, None)).await;
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(balance(&kit.services, client_id).await, 301);
    let mut conn = kit.services.store().conn().await.unwrap();
    let payments = conn.payments().for_order(order.id).await.unwrap();
    assert_eq!(payments.len(), 1);
    assert_eq!(payments[0].provider.as_str(), providers::BALANCE);
    assert_eq!(payments[0].status, PaymentStatus::Paid);
    let entries = conn
        .ledger()
        .entries(client_id, Default::default())
        .await
        .unwrap();
    assert_eq!(entries.items[0].kind, LedgerKind::Purchase);
    assert_eq!(entries.items[0].amount, Money::new(-699, Currency::Usd));
    assert_eq!(entries.items[0].order_id, Some(order.id));
    drop(conn);
    assert_eq!(
        jobs(&kit.services).await,
        vec![JobPayload::FulfillOrder { order_id: order.id }]
    );
    let events = events(&kit.services, client_id).await;
    assert!(events.iter().any(|e| matches!(
        e,
        DomainEvent::PaymentPaid { order_id, .. } if *order_id == order.id
    )));
    kit.db.cleanup().await;
}

#[tokio::test]
async fn бесплатный_заказ_сразу_оплачен() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 80).await;
    let tariff = standard(&kit.services).await;
    let mut conn = kit.services.store().conn().await.unwrap();
    let free = conn
        .tariffs()
        .create(&sn_store::NewTariff {
            traffic_addon: None,
            category_id: tariff.category_id,
            name: "Подарочный".into(),
            description: None,
            on_sale: true,
            sort: 1,
            devices: tariff.devices.clone(),
            traffic: TrafficPolicy::UNLIMITED,
            squads: tariff.squads.clone(),
            external_squad: None,
            periods: vec![sn_store::NewPeriod {
                days: 7,
                prices: sn_domain::tariff::CurrencyPrices {
                    usd: Some(0),
                    rub: Some(0),
                },
                stars: None,
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    drop(conn);
    let result = kit
        .services
        .checkout()
        .start(
            client_id,
            &request(
                CartRequest {
                    tariff_id: free.id,
                    period_id: free.periods[0].id,
                    extra_devices: 0,
                    subscription_id: None,
                    promo_code: None,
                },
                TEST_PAY,
            ),
        )
        .await
        .unwrap();
    let CheckoutResult::Completed { order_id } = result else {
        panic!("бесплатный заказ не должен вести на оплату");
    };
    let order = order_by_public_id(&kit.services, order_id).await;
    assert_eq!(order.status, OrderStatus::Paid);
    let mut conn = kit.services.store().conn().await.unwrap();
    let payments = conn.payments().for_order(order.id).await.unwrap();
    assert_eq!(payments[0].provider.as_str(), providers::MANUAL);
    assert_eq!(payments[0].amount.minor, 0);
    drop(conn);
    assert!(kit.pay.invoices.lock().unwrap().is_empty());
    assert_eq!(
        jobs(&kit.services).await,
        vec![JobPayload::FulfillOrder { order_id: order.id }]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пополнение_только_через_платёжные_системы_валюты() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 81).await;
    let checkout = kit.services.checkout();
    let amount = Money::new(1_000, Currency::Usd);
    assert_eq!(
        checkout
            .topup(client_id, amount, providers::BALANCE, None, Channel::Bot)
            .await
            .unwrap_err()
            .code(),
        "checkout.method_unavailable"
    );
    assert_eq!(
        checkout
            .topup(client_id, amount, providers::STARS, None, Channel::Bot)
            .await
            .unwrap_err()
            .code(),
        "checkout.method_unavailable"
    );
    assert_eq!(
        checkout
            .topup(
                client_id,
                Money::new(1_000, Currency::Rub),
                TEST_PAY,
                None,
                Channel::Bot
            )
            .await
            .unwrap_err()
            .code(),
        "topup.currency"
    );
    assert_eq!(
        checkout
            .topup(
                client_id,
                Money::new(50, Currency::Usd),
                TEST_PAY,
                None,
                Channel::Bot
            )
            .await
            .unwrap_err()
            .code(),
        "balance.topup_out_of_range"
    );
    let result = checkout
        .topup(client_id, amount, TEST_PAY, None, Channel::Bot)
        .await
        .unwrap();
    let CheckoutResult::Redirect { order_id, .. } = result else {
        panic!("пополнение идёт через счёт");
    };
    let order = order_by_public_id(&kit.services, order_id).await;
    assert_eq!(order.kind, OrderKind::TopUp);
    assert_eq!(order.intent, PurchaseIntent::TopUp { amount });
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пробный_период_один_раз_и_с_тегом() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 82).await;
    let order_id = kit
        .services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap();
    let order = order(&kit.services, order_id).await;
    assert_eq!(order.kind, OrderKind::Trial);
    assert_eq!(order.status, OrderStatus::Paid);
    let again = kit
        .services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap_err();
    assert_eq!(again.code(), "trial.used");
    let outcome = kit.services.fulfillment().run(order_id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order_id).await;
    let subscription = subscription(&kit.services, &order).await;
    assert_eq!(subscription.kind, SubscriptionKind::Trial);
    assert_eq!(subscription.tariff_id, None);
    assert!(!subscription.autorenew.enabled);
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.tag.as_deref(), Some(TRIAL_TAG));
    assert_eq!(user.hwid_device_limit, Some(1));
    assert_eq!(user.squad_ids(), vec![squad(9)]);
    let days = (user.expire_at - order.paid_at.unwrap()).num_hours();
    assert_eq!(days, 72);
    let after = kit
        .services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap_err();
    assert_eq!(after.code(), "trial.has_subscription");
    kit.db.cleanup().await;
}

fn place_squads(kit: &Kit, places: &[(u128, &str)]) {
    use sn_remnawave::fake::sample_node;
    use sn_remnawave::{Inbound, InternalSquad, NodeConfigProfile, NodeDetails, SquadInfo};
    let mut squads = Vec::new();
    let mut nodes = Vec::new();
    for n in [1u128, 2, 3, 9] {
        let place = places.iter().find(|(id, _)| *id == n);
        let inbound = Inbound {
            uuid: Uuid::from_u128(0x100 + n),
            profile_uuid: None,
            tag: format!("IN-{n}"),
            kind: "vless".into(),
            network: Some("tcp".into()),
            security: Some("reality".into()),
            port: Some(443),
        };
        squads.push(InternalSquad {
            uuid: squad(n).0,
            name: format!("Сквад {n}"),
            view_position: 0,
            info: SquadInfo::default(),
            inbounds: if place.is_some() {
                vec![inbound.clone()]
            } else {
                Vec::new()
            },
        });
        if let Some((_, country)) = place {
            let mut node = NodeDetails::from(sample_node(
                Uuid::from_u128(0x200 + n),
                &format!("N-{n}"),
                country,
            ));
            node.config_profile = Some(NodeConfigProfile {
                active_config_profile_uuid: None,
                active_inbounds: vec![inbound],
            });
            nodes.push(node);
        }
    }
    kit.remna.set_squads(squads);
    kit.remna.set_node_details(nodes);
}

#[tokio::test]
async fn вариант_пробного_периода_выбирает_клиент_и_пробник_один_на_всех() {
    let Some(kit) = kit().await else { return };
    let mut europe = trial_variant(4, "Европа", true, 1, 2, &[2]);
    europe.traffic = TrafficPolicy {
        limit_bytes: Some(10 * 1024 * 1024 * 1024),
        reset: sn_domain::traffic::TrafficReset::NoReset,
    };
    set_trial_variants(
        &kit,
        vec![
            trial_variant(1, "Россия", true, 3, 1, &[1]),
            europe,
            trial_variant(7, "Черновик", false, 5, 1, &[3]),
        ],
    )
    .await;
    place_squads(&kit, &[(1, "RU"), (2, "NL")]);
    let checkout = kit.services.checkout();
    let first = client(&kit.services, 2_710_000_001).await;
    let offer = checkout.trial_offer(first).await.unwrap();
    assert!(offer.available);
    let shown: Vec<(u32, &str, u32, u32)> = offer
        .variants
        .iter()
        .map(|v| (v.id, v.name.as_str(), v.days, v.devices))
        .collect();
    assert_eq!(shown, [(1, "Россия", 3, 1), (4, "Европа", 1, 2)]);
    assert_eq!(offer.variants[0].countries, ["RU"]);
    assert_eq!(offer.variants[1].countries, ["NL"]);
    assert_eq!(
        offer.variants[1].traffic.limit_bytes,
        Some(10 * 1024 * 1024 * 1024)
    );
    let code = |result: ServiceResult<OrderId>| result.unwrap_err().code();
    assert_eq!(
        code(checkout.start_trial(first, None).await),
        "trial.variant_required"
    );
    assert_eq!(
        code(checkout.start_trial(first, Some(7)).await),
        "trial.variant_unavailable"
    );
    assert_eq!(
        code(checkout.start_trial(first, Some(99)).await),
        "trial.variant_unavailable"
    );
    let order_id = checkout.start_trial(first, Some(4)).await.unwrap();
    let order = order(&kit.services, order_id).await;
    assert_eq!(order.kind, OrderKind::Trial);
    assert_eq!(order.intent, PurchaseIntent::Trial);
    assert_eq!(order.status, OrderStatus::Paid);
    assert!(order.paid_at.is_some());
    assert_eq!(order.quote.days, 1);
    let snapshot = order.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.name, "Европа");
    assert_eq!((snapshot.days, snapshot.devices_included), (1, 2));
    assert_eq!(snapshot.squads, vec![squad(2)]);
    assert_eq!(snapshot.traffic.limit_bytes, Some(10 * 1024 * 1024 * 1024));
    assert_eq!(
        code(checkout.start_trial(first, Some(1)).await),
        "trial.used"
    );
    let after = checkout.trial_offer(first).await.unwrap();
    assert!(!after.available);
    assert!(after.variants.is_empty());
    let outcome = kit.services.fulfillment().run(order_id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order_id).await;
    let subscription = subscription(&kit.services, &order).await;
    assert_eq!(subscription.kind, SubscriptionKind::Trial);
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.squad_ids(), vec![squad(2)]);
    assert_eq!(user.hwid_device_limit, Some(2));
    assert_eq!(user.traffic_limit_bytes, 10 * 1024 * 1024 * 1024);
    assert_eq!((user.expire_at - order.paid_at.unwrap()).num_hours(), 24);
    let second = client(&kit.services, 2_730_000_001).await;
    let order_id = checkout.start_trial(second, Some(1)).await.unwrap();
    let order = self::order(&kit.services, order_id).await;
    let snapshot = order.snapshot.as_ref().unwrap();
    assert_eq!((snapshot.name.as_str(), snapshot.days), ("Россия", 3));
    assert_eq!(snapshot.squads, vec![squad(1)]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn один_вариант_выдаётся_без_выбора_а_без_вариантов_пробника_нет() {
    let Some(kit) = kit().await else { return };
    set_trial_variants(
        &kit,
        vec![
            trial_variant(2, "", true, 2, 1, &[1]),
            trial_variant(3, "Выключен", false, 9, 1, &[2]),
        ],
    )
    .await;
    let checkout = kit.services.checkout();
    let only = client(&kit.services, 2_750_000_001).await;
    let offer = checkout.trial_offer(only).await.unwrap();
    assert_eq!(offer.variants.len(), 1);
    assert_eq!(offer.variants[0].name, "");
    let order_id = checkout.start_trial(only, None).await.unwrap();
    let order = order(&kit.services, order_id).await;
    let snapshot = order.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.name, "Пробный период");
    assert_eq!(snapshot.days, 2);
    set_trial_variants(&kit, vec![trial_variant(2, "", false, 2, 1, &[1])]).await;
    let late = client(&kit.services, 2_770_000_001).await;
    assert!(!checkout.trial_offer(late).await.unwrap().available);
    assert_eq!(
        checkout.start_trial(late, None).await.unwrap_err().code(),
        "trial.not_configured"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn покупка_после_пробного_периода_меняет_тариф_без_переноса_дней() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 83).await;
    let trial = kit
        .services
        .checkout()
        .start_trial(client_id, None)
        .await
        .unwrap();
    kit.services.fulfillment().run(trial).await.unwrap();
    let trial_order = order(&kit.services, trial).await;
    let trial_subscription = subscription(&kit.services, &trial_order).await;
    let tariff = standard(&kit.services).await;
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart(&tariff, 30, None))
        .await
        .unwrap();
    assert_eq!(view.kind, PlanKind::Change);
    credit(&kit.services, client_id, 1_000).await;
    let paid = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    assert_eq!(paid.kind, OrderKind::Change);
    assert_eq!(paid.subscription_id, Some(trial_subscription.id));
    kit.services.fulfillment().run(paid.id).await.unwrap();
    let paid = order(&kit.services, paid.id).await;
    assert_eq!(paid.status, OrderStatus::Fulfilled);
    let converted = subscription(&kit.services, &paid).await;
    assert_eq!(converted.kind, SubscriptionKind::Paid);
    assert_eq!(converted.tariff_id, Some(tariff.id));
    assert_eq!(converted.remna, trial_subscription.remna);
    let user = remna_user(&kit, &converted).await;
    assert_eq!(user.tag, None);
    assert_eq!(user.squad_ids(), vec![squad(1), squad(2)]);
    assert_eq!(user.hwid_device_limit, Some(3));
    assert_eq!(
        (user.expire_at - paid.paid_at.unwrap()).num_days(),
        30,
        "дни пробного периода не переносятся"
    );
    assert_eq!(kit.remna.users().len(), 1);
    kit.db.cleanup().await;
}

struct ExtraDevicesModule {
    fulfilled: Mutex<Vec<OrderId>>,
}

#[async_trait]
impl Module for ExtraDevicesModule {
    fn id(&self) -> ModuleId {
        ModuleId::Options
    }

    async fn quote_item(
        &self,
        _ctx: &ModuleContext,
        hook: &ItemHook,
    ) -> Result<Quote, ModuleError> {
        let total = Money::new(198, hook.currency);
        Ok(Quote {
            currency: hook.currency,
            lines: vec![QuoteLine {
                kind: QuoteLineKind::TopUp,
                label: "Доп. устройство на 2 месяца".into(),
                amount: total,
                stars: None,
            }],
            subtotal: total,
            discount: Money::zero(hook.currency),
            total,
            stars: None,
            days: 0,
            extra_devices: 1,
        })
    }

    async fn fulfill(&self, _ctx: &ModuleContext, order: &Order) -> Result<(), ModuleError> {
        self.fulfilled.lock().unwrap().push(order.id);
        Ok(())
    }
}

#[tokio::test]
async fn товар_модуля_продаётся_и_выдаётся_модулем() {
    let module = Arc::new(ExtraDevicesModule {
        fulfilled: Mutex::new(Vec::new()),
    });
    let Some(kit) = kit_with(vec![module.clone()]).await else {
        return;
    };
    let client_id = client(&kit.services, 84).await;
    credit(&kit.services, client_id, 500).await;
    let item = ItemRequest {
        module: ModuleId::Options,
        kind: "devices".into(),
        payload: serde_json::json!({"count": 1}),
        subscription_id: None,
    };
    let view = kit
        .services
        .checkout()
        .quote_item(client_id, &item)
        .await
        .unwrap();
    assert_eq!(view.quote.total, Money::new(198, Currency::Usd));
    let result = kit
        .services
        .checkout()
        .start_item(
            client_id,
            &ItemCheckoutRequest {
                item,
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Miniapp,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Completed { order_id } = result else {
        panic!("оплата с баланса проходит сразу");
    };
    let order = order_by_public_id(&kit.services, order_id).await;
    assert_eq!(order.kind, OrderKind::Module);
    assert_eq!(balance(&kit.services, client_id).await, 302);
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert_eq!(
        outcome,
        FulfillmentOutcome::Done {
            subscription_id: None
        }
    );
    assert_eq!(*module.fulfilled.lock().unwrap(), vec![order.id]);
    assert_eq!(
        self::order(&kit.services, order.id).await.status,
        OrderStatus::Fulfilled
    );
    kit.db.cleanup().await;
}

#[test]
fn адрес_возврата_принимается_только_свой_или_telegram() {
    let base = Some("https://panel.example.com");
    assert!(trusted_return_url(
        "https://panel.example.com/cabinet",
        base
    ));
    assert!(trusted_return_url("https://panel.example.com", base));
    assert!(trusted_return_url("https://t.me/stealth_bot/app", base));
    assert!(!trusted_return_url(
        "https://panel.example.com.evil.io/cabinet",
        base
    ));
    assert!(!trusted_return_url(
        "http://panel.example.com/cabinet",
        base
    ));
    assert!(!trusted_return_url("https://evil.io", base));
    assert!(!trusted_return_url("https://panel.example.com/x", None));
}
