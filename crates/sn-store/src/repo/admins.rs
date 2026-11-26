use crate::crypto::SealedSecret;
use crate::error::{StoreError, StoreResult};
use crate::repo::sessions::AdminLoginMethod;
use chrono::{DateTime, Utc};
use sn_domain::access::{AdminRole, Permission};
use sn_domain::ids::AdminId;
use sqlx::PgConnection;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRecord {
    pub id: AdminId,
    pub email: String,
    pub password_hash: String,
    pub role: AdminRole,
    pub permissions: Vec<Permission>,
    pub totp: Option<SealedSecret>,
    pub totp_last_step: Option<i64>,
    pub telegram_id: Option<i64>,
    pub must_change_password: bool,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdmin {
    pub email: String,
    pub password_hash: String,
    pub role: AdminRole,
    pub must_change_password: bool,
}

#[derive(sqlx::FromRow)]
struct AdminRow {
    id: i64,
    email: String,
    password_hash: String,
    role: String,
    permissions: Vec<String>,
    totp_secret: Option<Vec<u8>>,
    totp_nonce: Option<Vec<u8>>,
    totp_last_step: Option<i64>,
    telegram_id: Option<i64>,
    must_change_password: bool,
    disabled_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, email, password_hash, role, permissions, totp_secret, totp_nonce, totp_last_step, \
                       telegram_id, must_change_password, disabled_at, created_at";

impl TryFrom<AdminRow> for AdminRecord {
    type Error = StoreError;

    fn try_from(row: AdminRow) -> Result<Self, Self::Error> {
        let permissions = row
            .permissions
            .iter()
            .map(|p| Permission::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        let totp = match (row.totp_nonce, row.totp_secret) {
            (Some(nonce), Some(ciphertext)) => Some(SealedSecret { nonce, ciphertext }),
            _ => None,
        };
        Ok(AdminRecord {
            id: AdminId(row.id),
            email: row.email,
            password_hash: row.password_hash,
            role: AdminRole::parse(&row.role)?,
            permissions,
            totp,
            totp_last_step: row.totp_last_step,
            telegram_id: row.telegram_id,
            must_change_password: row.must_change_password,
            disabled_at: row.disabled_at,
            created_at: row.created_at,
        })
    }
}

pub struct AdminsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> AdminsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        AdminsRepo { conn }
    }

    pub async fn create(&mut self, admin: &NewAdmin) -> StoreResult<AdminRecord> {
        let row: AdminRow = sqlx::query_as(&format!(
            "INSERT INTO admins (email, password_hash, role, must_change_password) VALUES ($1, $2, $3, $4) RETURNING {COLUMNS}"
        ))
        .bind(admin.email.trim())
        .bind(&admin.password_hash)
        .bind(admin.role.as_str())
        .bind(admin.must_change_password)
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    pub async fn get(&mut self, id: AdminId) -> StoreResult<AdminRecord> {
        let row: AdminRow = sqlx::query_as(&format!("SELECT {COLUMNS} FROM admins WHERE id = $1"))
            .bind(id.0)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or(StoreError::NotFound("администратор"))?;
        row.try_into()
    }

    pub async fn by_email(&mut self, email: &str) -> StoreResult<Option<AdminRecord>> {
        let row: Option<AdminRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM admins WHERE lower(email) = lower($1)"
        ))
        .bind(email.trim())
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn list(&mut self) -> StoreResult<Vec<AdminRecord>> {
        let rows: Vec<AdminRow> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM admins ORDER BY id"))
                .fetch_all(&mut *self.conn)
                .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn count_active(&mut self) -> StoreResult<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM admins WHERE disabled_at IS NULL")
                .fetch_one(&mut *self.conn)
                .await?,
        )
    }

    pub async fn set_telegram(&mut self, id: AdminId, telegram_id: Option<i64>) -> StoreResult<()> {
        let done = sqlx::query("UPDATE admins SET telegram_id = $2 WHERE id = $1")
            .bind(id.0)
            .bind(telegram_id)
            .execute(&mut *self.conn)
            .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("администратор"));
        }
        Ok(())
    }

    pub async fn is_active_telegram(&mut self, telegram_id: i64) -> StoreResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM admins WHERE telegram_id = $1 AND disabled_at IS NULL)",
        )
        .bind(telegram_id)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn count_telegram_besides(&mut self, id: AdminId) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM admins WHERE telegram_id IS NOT NULL AND disabled_at IS NULL AND id <> $1",
        )
        .bind(id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn set_password(
        &mut self,
        id: AdminId,
        password_hash: &str,
        must_change: bool,
    ) -> StoreResult<()> {
        let done = sqlx::query(
            "UPDATE admins SET password_hash = $2, must_change_password = $3, disabled_at = NULL WHERE id = $1",
        )
        .bind(id.0)
        .bind(password_hash)
        .bind(must_change)
        .execute(&mut *self.conn)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("администратор"));
        }
        Ok(())
    }

    pub async fn set_role(
        &mut self,
        id: AdminId,
        role: AdminRole,
        permissions: &[Permission],
    ) -> StoreResult<()> {
        let permissions: Vec<&str> = permissions.iter().map(|p| p.as_str()).collect();
        sqlx::query("UPDATE admins SET role = $2, permissions = $3 WHERE id = $1")
            .bind(id.0)
            .bind(role.as_str())
            .bind(&permissions)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_totp(&mut self, id: AdminId, totp: Option<&SealedSecret>) -> StoreResult<()> {
        sqlx::query(
            "UPDATE admins SET totp_nonce = $2, totp_secret = $3, totp_last_step = NULL WHERE id = $1",
        )
        .bind(id.0)
        .bind(totp.map(|t| t.nonce.clone()))
        .bind(totp.map(|t| t.ciphertext.clone()))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn use_totp_step(&mut self, id: AdminId, step: i64) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE admins SET totp_last_step = $2 WHERE id = $1 AND (totp_last_step IS NULL OR totp_last_step < $2)",
        )
        .bind(id.0)
        .bind(step)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn disable(&mut self, id: AdminId) -> StoreResult<()> {
        sqlx::query("UPDATE admins SET disabled_at = now() WHERE id = $1 AND disabled_at IS NULL")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn enable(&mut self, id: AdminId) -> StoreResult<()> {
        sqlx::query("UPDATE admins SET disabled_at = NULL WHERE id = $1")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn lock_owners(&mut self) -> StoreResult<i64> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM admins WHERE role = 'owner' AND disabled_at IS NULL ORDER BY id FOR UPDATE",
        )
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(ids.len() as i64)
    }

    pub async fn last_logins(
        &mut self,
    ) -> StoreResult<HashMap<AdminId, (DateTime<Utc>, Option<AdminLoginMethod>)>> {
        let rows: Vec<(i64, DateTime<Utc>, Option<String>)> = sqlx::query_as(
            "SELECT DISTINCT ON (admin_id) admin_id, created_at, method FROM admin_sessions \
             ORDER BY admin_id, created_at DESC, id DESC",
        )
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, at, method)| {
                (
                    AdminId(id),
                    (at, method.as_deref().and_then(AdminLoginMethod::parse)),
                )
            })
            .collect())
    }

    pub async fn remember_login_address(
        &mut self,
        id: AdminId,
        address: &str,
        at: DateTime<Utc>,
        forget_before: DateTime<Utc>,
    ) -> StoreResult<()> {
        sqlx::query("DELETE FROM admin_login_addresses WHERE admin_id = $1 AND last_login_at < $2")
            .bind(id.0)
            .bind(forget_before)
            .execute(&mut *self.conn)
            .await?;
        sqlx::query(
            "INSERT INTO admin_login_addresses (admin_id, address, last_login_at) VALUES ($1, $2, $3) \
             ON CONFLICT (admin_id, address) DO UPDATE \
             SET last_login_at = GREATEST(admin_login_addresses.last_login_at, EXCLUDED.last_login_at)",
        )
        .bind(id.0)
        .bind(address)
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn logged_in_from(
        &mut self,
        email: &str,
        address: &str,
        since: DateTime<Utc>,
    ) -> StoreResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM admin_login_addresses k JOIN admins a ON a.id = k.admin_id \
             WHERE lower(a.email) = lower($1) AND k.address = $2 AND k.last_login_at > $3)",
        )
        .bind(email.trim())
        .bind(address)
        .bind(since)
        .fetch_one(&mut *self.conn)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::testing::test_store;
    use chrono::{Duration, TimeZone};

    async fn admin(db: &mut impl Db, email: &str) -> AdminId {
        db.admins()
            .create(&NewAdmin {
                email: email.into(),
                password_hash: "hash".into(),
                role: AdminRole::Owner,
                must_change_password: false,
            })
            .await
            .unwrap()
            .id
    }

    async fn addresses(db: &mut impl Db, id: AdminId) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT address FROM admin_login_addresses WHERE admin_id = $1 ORDER BY address",
        )
        .bind(id.0)
        .fetch_all(db.conn())
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn адрес_входа_запоминается_по_администратору_и_забывается_по_сроку() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let owner = admin(&mut conn, "Owner@Example.com").await;
        let other = admin(&mut conn, "support@example.com").await;
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap();
        let month = Duration::days(30);
        conn.admins()
            .remember_login_address(owner, "203.0.113.7", at, at - month)
            .await
            .unwrap();
        for (email, address, since, expected) in [
            ("owner@example.com", "203.0.113.7", at - month, true),
            (" OWNER@example.com ", "203.0.113.7", at - month, true),
            ("owner@example.com", "203.0.113.7", at, false),
            ("owner@example.com", "203.0.113.8", at - month, false),
            ("support@example.com", "203.0.113.7", at - month, false),
            ("nobody@example.com", "203.0.113.7", at - month, false),
        ] {
            assert_eq!(
                conn.admins()
                    .logged_in_from(email, address, since)
                    .await
                    .unwrap(),
                expected,
                "{email} {address} {since}"
            );
        }
        conn.admins()
            .remember_login_address(owner, "203.0.113.7", at - Duration::days(3), at - month)
            .await
            .unwrap();
        assert!(
            conn.admins()
                .logged_in_from("owner@example.com", "203.0.113.7", at - Duration::days(1))
                .await
                .unwrap()
        );
        conn.admins()
            .remember_login_address(other, "2001:db8:aaaa:1::/64", at, at - month)
            .await
            .unwrap();
        let later = at + Duration::days(31);
        conn.admins()
            .remember_login_address(owner, "198.51.100.9", later, later - month)
            .await
            .unwrap();
        assert_eq!(addresses(&mut conn, owner).await, ["198.51.100.9"]);
        assert_eq!(addresses(&mut conn, other).await, ["2001:db8:aaaa:1::/64"]);
        assert!(
            conn.admins()
                .remember_login_address(owner, "", later, later - month)
                .await
                .is_err()
        );
        drop(conn);
        db.cleanup().await;
    }
}
