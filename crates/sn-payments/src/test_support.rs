use crate::provider::{ClientRef, InvoiceRequest, ProviderConfig, WebhookRequest};
use bytes::Bytes;
use sn_domain::ids::{ClientId, PaymentId};
use sn_domain::money::{Amount, PayCurrency};
use uuid::Uuid;

pub(crate) const PAYMENT: Uuid = Uuid::from_u128(0x6a1c_0c8e_3f4b_4d2a_9b7e_51c0_2d3e_4f50);
pub(crate) const CLIENT: Uuid = Uuid::from_u128(0x0b9d_77aa_1c2d_4e3f_8a4b_5c6d_7e8f_9012);

pub(crate) fn usd(minor: i64) -> Amount {
    Amount::new(minor, PayCurrency::Usd)
}

pub(crate) fn rub(minor: i64) -> Amount {
    Amount::new(minor, PayCurrency::Rub)
}

pub(crate) fn config(pairs: &[(&str, serde_json::Value)]) -> ProviderConfig {
    let mut config = ProviderConfig::default();
    for (key, value) in pairs {
        config.set(key, value.clone());
    }
    config
}

pub(crate) fn invoice(amount: Amount, email: Option<&str>) -> InvoiceRequest {
    InvoiceRequest {
        plan: None,
        payment_id: PaymentId(41),
        payment_public_id: PAYMENT,
        order_public_id: Uuid::from_u128(7),
        amount,
        description: "Стандарт · 3 месяца".into(),
        client: ClientRef {
            client_id: ClientId(7),
            public_id: CLIENT,
            telegram_id: Some(5_093_118_422),
            email: email.map(str::to_string),
        },
        return_url: Some("https://panel.example.com/cabinet/payment-wait".into()),
        webhook_url: "https://panel.example.com/api/webhooks/pay/test".into(),
        expires_in: chrono::Duration::hours(1),
        save_payment_method: false,
        method: None,
    }
}

pub(crate) fn hook(headers: &[(&str, &str)], body: impl Into<Bytes>) -> WebhookRequest {
    WebhookRequest {
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.into(),
        remote_ip: None,
        route_suffix: None,
        query: None,
    }
}

pub(crate) fn closed_api() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

pub(crate) async fn failing_api(status: u16) -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(status).set_body_string("Bad Gateway"))
        .mount(&server)
        .await;
    server
}

pub(crate) const TEST_CA: &str = include_str!("providers/overpay/testdata/ca.pem");

pub(crate) async fn https_front(target: &str) -> String {
    use tokio_rustls::rustls;
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![
                CertificateDer::from_pem_slice(
                    include_str!("providers/overpay/testdata/server.crt").as_bytes(),
                )
                .unwrap(),
            ],
            PrivateKeyDer::from_pem_slice(
                include_str!("providers/overpay/testdata/server.key").as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let front = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let upstream = target
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(mut plain) = tokio::net::TcpStream::connect(upstream).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut tls, &mut plain).await;
            });
        }
    });
    front
}
