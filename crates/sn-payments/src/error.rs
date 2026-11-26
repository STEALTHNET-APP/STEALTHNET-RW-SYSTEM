pub type PaymentResult<T> = Result<T, PaymentError>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PaymentError {
    #[error("платёжная система не настроена: {0}")]
    NotConfigured(String),
    #[error("платёжная система не поддерживает валюту {0}")]
    UnsupportedCurrency(String),
    #[error("платёжная система этого не умеет: {0}")]
    Unsupported(&'static str),
    #[error("подключение платёжной системы ещё не реализовано: {0}")]
    NotImplemented(&'static str),
    #[error("подпись уведомления не совпала")]
    BadSignature,
    #[error("уведомление не разобрано: {0}")]
    BadPayload(String),
    #[error("платёжная система отклонила запрос: {0}")]
    Rejected(String),
    #[error("платёжная система ответила ошибкой HTTP {status}")]
    Http { status: u16 },
    #[error("платёжная система недоступна: {0}")]
    Transport(String),
    #[error("для оплаты этим способом нужна почта покупателя")]
    EmailRequired,
}

impl PaymentError {
    pub fn invoice_failure(&self) -> String {
        let cause = match self {
            PaymentError::BadPayload(text) | PaymentError::Rejected(text) => text.clone(),
            PaymentError::Http { status } => {
                format!("платёжная система ответила ошибкой HTTP {status}")
            }
            other => other.to_string(),
        };
        format!("Счёт не выставлен: {cause}")
    }

    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            PaymentError::Transport(_) | PaymentError::Http { status: 500..=599 }
        )
    }

    pub const fn is_authentication(&self) -> bool {
        matches!(self, PaymentError::BadSignature)
    }

    pub fn transport(err: reqwest::Error) -> Self {
        let err = err.without_url();
        let mut cause: Option<String> = None;
        let mut source = std::error::Error::source(&err);
        while let Some(inner) = source {
            cause = Some(inner.to_string());
            source = inner.source();
        }
        let reason = match (err.is_connect(), err.is_timeout(), cause) {
            (true, true, _) => "сервер не ответил на соединение".to_string(),
            (true, false, Some(cause)) => format!("не удалось соединиться с сервером ({cause})"),
            (true, false, None) => "не удалось соединиться с сервером".to_string(),
            (false, true, _) => "истекло время ожидания ответа".to_string(),
            (false, false, Some(cause)) => format!("{err} ({cause})"),
            (false, false, None) => err.to_string(),
        };
        PaymentError::Transport(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn обрыв_соединения_объясняется_по_русски() {
        let url = format!(
            "{}/api/getMe?token=secret",
            crate::test_support::closed_api()
        );
        let err = reqwest::Client::new().get(&url).send().await.unwrap_err();
        let PaymentError::Transport(reason) = PaymentError::transport(err) else {
            panic!("ожидалась ошибка соединения");
        };
        assert!(
            reason.starts_with("не удалось соединиться с сервером"),
            "{reason}"
        );
        assert!(!reason.contains("secret"), "{reason}");
        assert!(!reason.contains("127.0.0.1"), "{reason}");
    }
}
