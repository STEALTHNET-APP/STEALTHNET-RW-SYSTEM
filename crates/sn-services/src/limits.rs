use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, TimeZone, Utc};
use sha2::{Digest, Sha256};
use sn_modules::catalog::antibot;
use sn_store::Db;
use std::net::{IpAddr, Ipv6Addr};

pub const RATE_LIMITED: &str = "auth.rate_limited";
pub const RETENTION_DAYS: i64 = 2;
pub const UNKNOWN_ADDRESS: &str = "unknown";
pub const IPV6_NETWORK_BITS: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRule {
    pub scope: &'static str,
    pub limit: u32,
    pub window_seconds: i64,
}

const ROOM: u32 = 3;

pub mod rules {
    use super::RateRule;
    use sn_modules::catalog::antibot;

    pub const ADMIN_LOGIN_IP: RateRule = RateRule {
        scope: "admin_login_ip",
        limit: 30,
        window_seconds: 300,
    };
    pub const ADMIN_LOGIN_ACCOUNT: RateRule = RateRule {
        scope: "admin_login_account",
        limit: 20,
        window_seconds: 900,
    };
    pub const ADMIN_LOGIN_ACCOUNT_IP: RateRule = RateRule {
        scope: "admin_login_account_ip",
        limit: 10,
        window_seconds: 900,
    };
    pub const ADMIN_TOTP_FAILURES: RateRule = RateRule {
        scope: "admin_totp_failures",
        limit: 5,
        window_seconds: 900,
    };
    pub const ADMIN_TOTP_FAILURES_DAY: RateRule = RateRule {
        scope: "admin_totp_failures_day",
        limit: 20,
        window_seconds: 86_400,
    };
    pub const ADMIN_SECURITY: RateRule = RateRule {
        scope: "admin_security",
        limit: 10,
        window_seconds: 900,
    };
    pub const MINIAPP_IP: RateRule = RateRule {
        scope: "miniapp_login_ip",
        limit: 120,
        window_seconds: 60,
    };
    pub const TELEGRAM_START_IP: RateRule = RateRule {
        scope: "telegram_login_ip",
        limit: antibot::TELEGRAM_IP,
        window_seconds: 600,
    };
    pub const TELEGRAM_POLL_IP: RateRule = RateRule {
        scope: "telegram_poll_ip",
        limit: 120,
        window_seconds: 60,
    };
    pub const EMAIL_START_IP: RateRule = RateRule {
        scope: "email_login_ip",
        limit: antibot::EMAIL_IP,
        window_seconds: 600,
    };
    pub const EMAIL_START_ADDRESS: RateRule = RateRule {
        scope: "email_login_address",
        limit: 3,
        window_seconds: 900,
    };
    pub const EMAIL_VERIFY_IP: RateRule = RateRule {
        scope: "email_verify_ip",
        limit: 20,
        window_seconds: 900,
    };
    pub const EMAIL_VERIFY_ADDRESS: RateRule = RateRule {
        scope: "email_verify_address",
        limit: 10,
        window_seconds: 900,
    };
    pub const EMAIL_START_WIDE: RateRule = RateRule {
        scope: "email_login_wide",
        limit: 15,
        window_seconds: 600,
    };
    pub const EMAIL_CODE_FAILURES_DAY: RateRule = RateRule {
        scope: "email_code_failures_day",
        limit: 30,
        window_seconds: 86_400,
    };
    pub const CLIENT_TOTP_IP: RateRule = RateRule {
        scope: "client_totp_ip",
        limit: 30,
        window_seconds: 900,
    };
    pub const CLIENT_TOTP_FAILURES: RateRule = RateRule {
        scope: "client_totp_failures",
        limit: 5,
        window_seconds: 900,
    };
    pub const CLIENT_TOTP_FAILURES_DAY: RateRule = RateRule {
        scope: "client_totp_failures_day",
        limit: 20,
        window_seconds: 86_400,
    };
    pub const CLIENT_SECURITY: RateRule = RateRule {
        scope: "client_security",
        limit: 10,
        window_seconds: 900,
    };
    pub const EMAIL_CHANGE_CLIENT: RateRule = RateRule {
        scope: "email_change_client",
        limit: 5,
        window_seconds: 900,
    };
    pub const OAUTH_START_IP: RateRule = RateRule {
        scope: "oauth_start_ip",
        limit: 20,
        window_seconds: 600,
    };
    pub const OAUTH_FINISH_IP: RateRule = RateRule {
        scope: "oauth_finish_ip",
        limit: 60,
        window_seconds: 600,
    };
    pub const TELEGRAM_LINK_CLIENT: RateRule = RateRule {
        scope: "telegram_link_client",
        limit: 10,
        window_seconds: 900,
    };
    pub const TELEGRAM_LINK_IP: RateRule = RateRule {
        scope: "telegram_link_ip",
        limit: 20,
        window_seconds: 600,
    };
    pub const TELEGRAM_LINK_USER: RateRule = RateRule {
        scope: "telegram_link_user",
        limit: 20,
        window_seconds: 600,
    };
    pub const CAPTCHA_IP: RateRule = RateRule {
        scope: "captcha_ip",
        limit: 30,
        window_seconds: 600,
    };
    pub const VERIFY_PAGE_IP: RateRule = RateRule {
        scope: "verify_page_ip",
        limit: antibot::VERIFY_IP,
        window_seconds: 600,
    };
    pub const APP_LINK_CLIENT: RateRule = RateRule {
        scope: "app_link_client",
        limit: 30,
        window_seconds: 600,
    };
    pub const APP_LINK_IP: RateRule = RateRule {
        scope: "app_link_ip",
        limit: 120,
        window_seconds: 60,
    };
    pub const PAYMENT_CHECK: RateRule = RateRule {
        scope: "payment_check",
        limit: 10,
        window_seconds: 600,
    };
    pub const PAYMENT_CATALOG: RateRule = RateRule {
        scope: "payment_catalog",
        limit: 30,
        window_seconds: 600,
    };
}

pub fn ip_key(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let network = u128::from(v6) & (u128::MAX << (128 - IPV6_NETWORK_BITS));
            format!("{}/{IPV6_NETWORK_BITS}", Ipv6Addr::from(network))
        }
    }
}

pub const IPV6_WIDE_BITS: u32 = 48;

pub fn wide_key(ip: Option<&str>) -> Option<String> {
    match ip?.trim().parse::<IpAddr>().ok()?.to_canonical() {
        IpAddr::V4(_) => None,
        IpAddr::V6(v6) => {
            let network = u128::from(v6) & (u128::MAX << (128 - IPV6_WIDE_BITS));
            Some(format!("{}/{IPV6_WIDE_BITS}", Ipv6Addr::from(network)))
        }
    }
}

pub fn code_letters(kind: &'static str, per_hour: u32) -> RateRule {
    RateRule {
        scope: kind,
        limit: per_hour,
        window_seconds: 3_600,
    }
}

pub fn network_key(ip: Option<&str>) -> Option<String> {
    ip?.trim().parse::<IpAddr>().ok().map(ip_key)
}

pub fn address_key(ip: Option<&str>) -> String {
    network_key(ip).unwrap_or_else(|| {
        ip.map(|raw| raw.trim().to_lowercase())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| UNKNOWN_ADDRESS.to_string())
    })
}

pub fn bucket(rule: RateRule, identity: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(rule.scope.as_bytes());
    hasher.update(b"\n");
    hasher.update(identity.trim().to_lowercase().as_bytes());
    hasher.finalize().to_vec()
}

pub fn window_start(rule: RateRule, now: DateTime<Utc>) -> DateTime<Utc> {
    let window = rule.window_seconds.max(1);
    let start = now.timestamp().div_euclid(window) * window;
    Utc.timestamp_opt(start, 0).single().unwrap_or(now)
}

fn window(rule: RateRule) -> Duration {
    Duration::seconds(rule.window_seconds.max(1))
}

pub fn wait_phrase(left: Duration) -> String {
    let minutes = (left.num_seconds().max(1) + 59) / 60;
    if minutes <= 90 {
        format!("{minutes} мин.")
    } else {
        format!("{} ч.", (minutes + 59) / 60)
    }
}

pub fn rate_limited(reason: &str, left: Duration) -> ServiceError {
    ServiceError::rejected(
        RATE_LIMITED,
        format!("{reason} Попробуйте снова через {}", wait_phrase(left)),
    )
}

fn too_many(rule: RateRule, start: DateTime<Utc>, now: DateTime<Utc>) -> ServiceError {
    rate_limited("Слишком много попыток.", start + window(rule) - now)
}

pub fn unlock_at(rule: RateRule, start: DateTime<Utc>, current: u32) -> DateTime<Utc> {
    if current >= rule.limit {
        start + window(rule) + window(rule)
    } else {
        start + window(rule)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Strike {
    bucket: Vec<u8>,
    window_start: DateTime<Utc>,
    pub rule: RateRule,
    pub failures: u32,
}

impl Strike {
    pub const fn reached_limit(&self) -> bool {
        self.failures == self.rule.limit
    }
}

pub struct RateLimits<'a> {
    services: &'a Services,
}

impl<'a> RateLimits<'a> {
    pub fn new(services: &'a Services) -> Self {
        RateLimits { services }
    }

    fn paces(&self) -> antibot::Paces {
        antibot::paces(self.services)
    }

    pub fn email_start(&self) -> RateRule {
        RateRule {
            limit: self.paces().email,
            ..rules::EMAIL_START_IP
        }
    }

    pub fn email_start_wide(&self) -> RateRule {
        self.roomy(rules::EMAIL_START_WIDE)
    }

    pub fn captcha(&self) -> RateRule {
        self.roomy(rules::CAPTCHA_IP)
    }

    fn roomy(&self, rule: RateRule) -> RateRule {
        RateRule {
            limit: rule.limit.max(self.paces().email.saturating_mul(ROOM)),
            ..rule
        }
    }

    pub fn telegram_start(&self) -> RateRule {
        RateRule {
            limit: self.paces().telegram,
            ..rules::TELEGRAM_START_IP
        }
    }

    pub fn verify_page(&self) -> RateRule {
        RateRule {
            limit: self.paces().verify,
            ..rules::VERIFY_PAGE_IP
        }
    }

    async fn sweep(&self, conn: &mut sn_store::Conn, now: DateTime<Utc>) -> ServiceResult<()> {
        sqlx::query("DELETE FROM auth_rate_limits WHERE window_start < $1")
            .bind(now - Duration::days(RETENTION_DAYS))
            .execute(conn.conn())
            .await?;
        Ok(())
    }

    pub async fn hit(&self, rule: RateRule, identity: &str) -> ServiceResult<u32> {
        let now = self.services.now();
        let start = window_start(rule, now);
        let mut conn = self.services.store().conn().await?;
        let hits: i32 = sqlx::query_scalar(
            "INSERT INTO auth_rate_limits (bucket, window_start, hits) VALUES ($1, $2, 1) \
             ON CONFLICT (bucket, window_start) DO UPDATE SET hits = LEAST(auth_rate_limits.hits + 1, 1000000) \
             RETURNING hits",
        )
        .bind(bucket(rule, identity))
        .bind(start)
        .fetch_one(conn.conn())
        .await?;
        if hits == 1 {
            self.sweep(&mut conn, now).await?;
        }
        let hits = u32::try_from(hits).unwrap_or(u32::MAX);
        if hits > rule.limit {
            return Err(too_many(rule, start, now));
        }
        Ok(hits)
    }

    pub async fn hit_ip(&self, rule: RateRule, ip: Option<&str>) -> ServiceResult<u32> {
        self.hit(rule, &address_key(ip)).await
    }

    pub async fn hit_wide(&self, rule: RateRule, ip: Option<&str>) -> ServiceResult<()> {
        if let Some(key) = wide_key(ip) {
            self.hit(rule, &key).await?;
        }
        Ok(())
    }

    pub async fn letters_sent(&self, kind: &'static str, per_hour: u32) -> ServiceResult<()> {
        let now = self.services.now();
        let rule = code_letters(kind, per_hour);
        let start = window_start(rule, now);
        match self.hit(rule, "panel").await {
            Ok(_) => Ok(()),
            Err(_) => Err(rate_limited(
                "Сейчас отправлено слишком много писем с кодами.",
                start + window(rule) - now,
            )),
        }
    }

    pub async fn strike(
        &self,
        rule: RateRule,
        identity: &str,
        reason: &str,
    ) -> ServiceResult<Strike> {
        let now = self.services.now();
        let start = window_start(rule, now);
        let strike_bucket = bucket(rule, identity);
        let mut conn = self.services.store().conn().await?;
        let (current, previous): (i32, i32) = sqlx::query_as(
            "WITH cur AS ( \
                 INSERT INTO auth_rate_limits (bucket, window_start, hits) VALUES ($1, $2, 1) \
                 ON CONFLICT (bucket, window_start) DO UPDATE SET hits = LEAST(auth_rate_limits.hits + 1, 1000000) \
                 RETURNING hits) \
             SELECT (SELECT hits FROM cur), \
                    COALESCE((SELECT hits FROM auth_rate_limits WHERE bucket = $1 AND window_start = $3), 0)",
        )
        .bind(&strike_bucket)
        .bind(start)
        .bind(start - window(rule))
        .fetch_one(conn.conn())
        .await?;
        if current == 1 {
            self.sweep(&mut conn, now).await?;
        }
        let current = u32::try_from(current).unwrap_or(u32::MAX);
        let previous = u32::try_from(previous).unwrap_or(u32::MAX);
        let strike = Strike {
            bucket: strike_bucket,
            window_start: start,
            rule,
            failures: current.saturating_add(previous),
        };
        if strike.failures > rule.limit {
            self.release(&mut conn, &strike).await?;
            let left = unlock_at(rule, start, current.saturating_sub(1)) - now;
            return Err(rate_limited(reason, left));
        }
        Ok(strike)
    }

    pub(crate) async fn release(&self, conn: &mut impl Db, strike: &Strike) -> ServiceResult<()> {
        sqlx::query(
            "WITH gone AS ( \
                 DELETE FROM auth_rate_limits WHERE bucket = $1 AND window_start = $2 AND hits <= 1 RETURNING 1) \
             UPDATE auth_rate_limits SET hits = hits - 1 WHERE bucket = $1 AND window_start = $2 AND hits > 1",
        )
        .bind(&strike.bucket)
        .bind(strike.window_start)
        .execute(conn.conn())
        .await?;
        Ok(())
    }

    pub async fn forgive(&self, strike: &Strike) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        self.release(&mut conn, strike).await
    }

    pub async fn forget(&self, rule: RateRule, identity: &str) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        sqlx::query("DELETE FROM auth_rate_limits WHERE bucket = $1")
            .bind(bucket(rule, identity))
            .execute(conn.conn())
            .await?;
        Ok(())
    }

    pub async fn failures(&self, rule: RateRule, identity: &str) -> ServiceResult<u32> {
        let start = window_start(rule, self.services.now());
        let mut conn = self.services.store().conn().await?;
        let total: i64 = sqlx::query_scalar(
            "SELECT coalesce(sum(hits), 0)::bigint FROM auth_rate_limits \
             WHERE bucket = $1 AND window_start IN ($2, $3)",
        )
        .bind(bucket(rule, identity))
        .bind(start)
        .bind(start - window(rule))
        .fetch_one(conn.conn())
        .await?;
        Ok(u32::try_from(total).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn окно_выравнивается_по_длине() {
        let rule = rules::ADMIN_LOGIN_IP;
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 31).unwrap();
        assert_eq!(
            window_start(rule, at),
            Utc.with_ymd_and_hms(2026, 11, 16, 14, 50, 0).unwrap()
        );
        assert_eq!(bucket(rule, " 203.0.113.7 "), bucket(rule, "203.0.113.7"));
        assert_ne!(
            bucket(rule, "203.0.113.7"),
            bucket(rules::TELEGRAM_POLL_IP, "203.0.113.7")
        );
        let err = too_many(rule, window_start(rule, at), at);
        assert_eq!(err.code(), RATE_LIMITED);
        assert!(err.to_string().contains("3 мин"));
    }

    #[test]
    fn адрес_ipv6_сводится_к_сети_64_а_ipv4_остаётся_целым() {
        assert_eq!(address_key(Some("2001:db8:1:2::a")), "2001:db8:1:2::/64");
        assert_eq!(
            address_key(Some("2001:DB8:1:2:ffff:ffff:ffff:ffff")),
            "2001:db8:1:2::/64"
        );
        assert_eq!(address_key(Some("2001:db8:1:3::a")), "2001:db8:1:3::/64");
        assert_eq!(address_key(Some(" 203.0.113.7 ")), "203.0.113.7");
        assert_eq!(address_key(Some("203.0.113.8")), "203.0.113.8");
        assert_eq!(address_key(Some("::ffff:203.0.113.7")), "203.0.113.7");
        assert_eq!(address_key(Some("::1")), "::/64");
        assert_eq!(address_key(None), UNKNOWN_ADDRESS);
        assert_eq!(address_key(Some("  ")), UNKNOWN_ADDRESS);
        assert_eq!(address_key(Some("Proxy-A")), "proxy-a");
        assert_eq!(
            network_key(Some("2001:db8:1:2::a")).as_deref(),
            Some("2001:db8:1:2::/64")
        );
        assert_eq!(network_key(Some("proxy-a")), None);
        assert_eq!(network_key(None), None);
        let link_local: IpAddr = "fe80::1c2:3ff:fe04:506".parse().unwrap();
        assert_eq!(ip_key(link_local), "fe80::/64");
        assert_eq!(
            bucket(rules::ADMIN_LOGIN_IP, &address_key(Some("2001:db8:1:2::1"))),
            bucket(rules::ADMIN_LOGIN_IP, &address_key(Some("2001:db8:1:2::a")))
        );
    }

    #[test]
    fn общий_лимит_неудач_входа_двадцать_за_пятнадцать_минут() {
        assert_eq!(rules::ADMIN_LOGIN_ACCOUNT.limit, 20);
        assert_eq!(rules::ADMIN_LOGIN_ACCOUNT.window_seconds, 900);
        assert_eq!(rules::ADMIN_LOGIN_ACCOUNT_IP.limit, 10);
        assert_eq!(rules::ADMIN_LOGIN_ACCOUNT_IP.window_seconds, 900);
    }

    #[test]
    fn блокировка_после_неудач_держится_до_конца_следующего_окна() {
        let rule = rules::ADMIN_TOTP_FAILURES;
        let start = Utc.with_ymd_and_hms(2026, 11, 16, 14, 45, 0).unwrap();
        assert_eq!(
            unlock_at(rule, start, 2),
            Utc.with_ymd_and_hms(2026, 11, 16, 15, 0, 0).unwrap()
        );
        assert_eq!(
            unlock_at(rule, start, rule.limit),
            Utc.with_ymd_and_hms(2026, 11, 16, 15, 15, 0).unwrap()
        );
        assert_eq!(wait_phrase(Duration::seconds(30)), "1 мин.");
        assert_eq!(wait_phrase(Duration::minutes(90)), "90 мин.");
        assert_eq!(wait_phrase(Duration::hours(30)), "30 ч.");
        assert!(
            rate_limited("Слишком много попыток.", Duration::minutes(20))
                .to_string()
                .ends_with("через 20 мин.")
        );
    }
}
