use super::*;
use sn_domain::pricing::bonus_taken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RefundBonus {
    pub left: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Hold {
    pub entry: LedgerEntryId,
    pub bonus_minor: i64,
}

pub(super) fn shown(minor: i64, amount: Amount) -> Option<Amount> {
    (minor > 0).then(|| Amount::new(minor, amount.currency))
}

async fn change(
    tx: &mut Tx,
    payment: &Payment,
    order: OrderId,
    amount: Money,
    kind: LedgerKind,
    key: String,
    comment: Option<String>,
) -> ServiceResult<bool> {
    let appended = tx
        .ledger()
        .append(&NewLedgerEntry {
            client_id: payment.client_id,
            wallet: Wallet::Main,
            amount,
            kind,
            order_id: Some(order),
            payment_id: Some(payment.id),
            admin_id: None,
            comment,
            idempotency_key: Some(key),
        })
        .await?;
    if appended.duplicate {
        return Ok(false);
    }
    tx.events()
        .push(&DomainEvent::BalanceChanged {
            client_id: payment.client_id,
            wallet: Wallet::Main,
            kind,
            delta: amount,
        })
        .await?;
    Ok(true)
}

pub(super) async fn credit(
    tx: &mut Tx,
    order: &Order,
    paid: &Payment,
    amount: Money,
) -> ServiceResult<Option<Money>> {
    let Some(terms) = tx.payments().bonus(paid.id).await?.terms else {
        return Ok(None);
    };
    let full = terms.bonus(amount);
    let refunded = refunded_minor(tx, paid.id).await?;
    let bonus = Money::new(
        full.minor - bonus_taken(full.minor, amount.minor, refunded),
        amount.currency,
    );
    if bonus.is_zero() {
        return Ok(None);
    }
    let credited = change(
        tx,
        paid,
        order.id,
        bonus,
        LedgerKind::TopupBonus,
        format!("topup_bonus:payment:{}", paid.id),
        Some(format!("Бонус {}% к пополнению", terms.percent)),
    )
    .await?;
    if !credited {
        return Ok(None);
    }
    tx.payments().bonus_credited(paid.id, bonus.minor).await?;
    Ok(Some(bonus))
}

async fn taken_minor(
    db: &mut impl Db,
    payment: PaymentId,
    except: Option<i64>,
) -> ServiceResult<(i64, i64)> {
    let mut refunded = 0;
    let mut taken = 0;
    for refund in db.refunds().for_payment(payment).await? {
        refunded += refund.amount.minor;
        taken += refund.bonus_minor;
    }
    let requests = db.payments().provider_refunds(payment).await?;
    for request in pending_requests(&requests).filter(|r| Some(r.id) != except) {
        refunded += request.amount.minor;
        taken += request.bonus_minor;
    }
    Ok((refunded, taken))
}

pub(super) async fn due(
    tx: &mut Tx,
    payment: &Payment,
    minor: i64,
    except: Option<i64>,
) -> ServiceResult<i64> {
    let credited = tx.payments().bonus(payment.id).await?.credited_minor;
    if credited == 0 {
        return Ok(0);
    }
    let (refunded, taken) = taken_minor(tx, payment.id, except).await?;
    Ok(bonus_taken(
        credited - taken,
        payment.amount.minor - refunded,
        minor,
    ))
}

pub(super) async fn left(
    db: &mut impl Db,
    payment: &Payment,
) -> ServiceResult<Option<RefundBonus>> {
    let credited = db.payments().bonus(payment.id).await?.credited_minor;
    let Some(paid) = payment.amount.as_money().filter(|_| credited > 0) else {
        return Ok(None);
    };
    let (_, taken) = taken_minor(db, payment.id, None).await?;
    let left = credited - taken;
    Ok((left > 0).then(|| RefundBonus {
        left: Money::new(left, paid.currency),
    }))
}

pub(super) async fn take(
    tx: &mut Tx,
    payment: &Payment,
    order: OrderId,
    bonus: Money,
    key: String,
) -> ServiceResult<()> {
    if bonus.minor <= 0 {
        return Ok(());
    }
    change(
        tx,
        payment,
        order,
        bonus.checked_neg()?,
        LedgerKind::TopupBonusReversal,
        key,
        Some(format!(
            "Бонус за пополнение снят: возврат платежа {}",
            payment.public_id
        )),
    )
    .await?;
    Ok(())
}

pub(super) async fn release(
    tx: &mut Tx,
    payment: &Payment,
    request: i64,
    bonus: Money,
) -> ServiceResult<()> {
    if bonus.minor <= 0 {
        return Ok(());
    }
    change(
        tx,
        payment,
        payment.order_id,
        bonus,
        LedgerKind::TopupBonus,
        format!("topup_bonus_release:{request}"),
        Some(format!(
            "Бонус за пополнение вернулся: возврат платежа {} не состоялся",
            payment.public_id
        )),
    )
    .await?;
    Ok(())
}
