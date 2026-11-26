pub const UNIQUE_FIELDS: &[(&str, Option<&str>)] = &[
    ("admins_email_key", Some("Почта")),
    ("admin_sessions_token_key", None),
    ("api_keys_token_key", None),
    ("ext_modules_api_key_idx", None),
    ("ext_module_deliveries_event_key", None),
    ("clients_public_id_key", None),
    (
        "client_identities_kind_value_key",
        Some("Telegram или почта"),
    ),
    ("client_sessions_token_key", None),
    ("tariff_periods_tariff_days_key", Some("Срок в днях")),
    ("subscriptions_public_id_key", None),
    (
        "subscriptions_remna_user_key",
        Some("Пользователь Remnawave"),
    ),
    (
        "subscriptions_remna_username_key",
        Some("Имя пользователя Remnawave"),
    ),
    ("orders_public_id_key", None),
    ("payments_public_id_key", None),
    ("payments_provider_txid_key", Some("Номер транзакции")),
    ("saved_payment_methods_active_key", None),
    ("balance_entries_idempotency_key", None),
    ("autorenew_attempts_key", None),
    ("jobs_dedupe_key", None),
    ("webhook_inbox_source_dedupe_key", None),
    ("remna_events_body_key", None),
    ("promo_codes_code_key", Some("Промокод")),
    ("utm_links_tags_key", Some("Метки ссылки")),
    ("promo_redemptions_order_key", None),
    ("referral_codes_code_key", None),
    ("referral_credits_payment_level_key", None),
    ("referral_withdrawals_public_id_key", None),
    ("broadcast_recipients_chat_key", None),
    ("broadcast_images_public_key", None),
    ("broadcast_recipients_email_key", None),
    ("broadcast_recipients_unsubscribe_key", None),
    ("broadcasts_rule_day_key", None),
    ("auto_broadcasts_preset_key", None),
    ("broadcasts_contest_message_key", None),
    ("contest_entries_place_key", None),
    ("roulette_tickets_order_key", None),
    ("roulette_spins_request_key", None),
    ("roulette_spins_ticket_key", None),
    ("import_runs_public_id_key", None),
    ("import_runs_running_key", None),
    ("remna_transfers_public_id_key", None),
    ("remna_transfers_busy_key", None),
    ("legacy_invoices_old_key", None),
    ("gifts_public_id_key", None),
    ("gifts_code_key", None),
    ("gifts_order_key", None),
    ("promo_links_code_key", Some("Код ссылки")),
    ("promo_link_activations_client_key", None),
    ("promo_link_activations_order_key", None),
    ("payment_notices_event_key", None),
    ("refunds_request_key", None),
    ("ticket_files_link_key", None),
    ("provider_refunds_request_key", None),
    ("antibot_events_dedupe_key", None),
    ("traffic_exceeds_window_key", None),
    ("oauth_requests_state_key", None),
    ("email_signups_token_key", None),
    ("antibot_checks_token_key", None),
    (
        "client_identities_one_oauth_idx",
        Some("Вход через Google или Apple"),
    ),
    ("backup_runs_slot_key", None),
    ("provider_subscriptions_external_key", None),
    ("provider_subscriptions_live_key", None),
    ("provider_subscription_charges_payment_key", None),
    ("provider_subscription_charges_external_key", None),
];

pub fn field_for(constraint: &str) -> Option<&'static str> {
    UNIQUE_FIELDS
        .iter()
        .find(|(name, _)| *name == constraint)
        .and_then(|(_, field)| *field)
}

pub fn is_known(constraint: &str) -> bool {
    UNIQUE_FIELDS.iter().any(|(name, _)| *name == constraint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct Found {
        named: BTreeSet<String>,
        unnamed: Vec<String>,
    }

    fn tokens(sql: &str) -> Vec<String> {
        sql.lines()
            .map(|line| line.split("--").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ")
            .replace(['(', ')', ',', ';'], " ")
            .split_whitespace()
            .map(|t| t.trim_matches('"').to_string())
            .collect()
    }

    fn scan(sql: &str, file: &str, found: &mut Found) {
        let tokens = tokens(sql);
        let upper: Vec<String> = tokens.iter().map(|t| t.to_ascii_uppercase()).collect();
        for (i, token) in upper.iter().enumerate() {
            if token != "UNIQUE" {
                continue;
            }
            if i >= 2 && upper[i - 2] == "CONSTRAINT" {
                found.named.insert(tokens[i - 1].clone());
                continue;
            }
            if i >= 1
                && upper[i - 1] == "CREATE"
                && upper.get(i + 1).map(String::as_str) == Some("INDEX")
            {
                let name = tokens
                    .iter()
                    .zip(upper.iter())
                    .skip(i + 2)
                    .find(|(_, u)| !matches!(u.as_str(), "CONCURRENTLY" | "IF" | "NOT" | "EXISTS"))
                    .map(|(t, _)| t.clone());
                match name {
                    Some(name) => {
                        found.named.insert(name);
                    }
                    None => found
                        .unnamed
                        .push(format!("{file}: UNIQUE INDEX без имени")),
                }
                continue;
            }
            let context = tokens[i.saturating_sub(3)..(i + 1).min(tokens.len())].join(" ");
            found.unnamed.push(format!("{file}: {context}"));
        }
    }

    fn migrations() -> Found {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
        let mut found = Found {
            named: BTreeSet::new(),
            unnamed: Vec::new(),
        };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "миграции не найдены в {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).unwrap();
            let file = path.file_name().unwrap().to_string_lossy().to_string();
            scan(&sql, &file, &mut found);
        }
        found
    }

    #[test]
    fn каждое_уникальное_ограничение_из_миграций_есть_в_словаре() {
        let found = migrations();
        assert!(
            found.unnamed.is_empty(),
            "у уникальных ограничений должно быть явное имя: {:?}",
            found.unnamed
        );
        let missing: Vec<&String> = found.named.iter().filter(|name| !is_known(name)).collect();
        assert!(
            missing.is_empty(),
            "добавьте в UNIQUE_FIELDS (имя поля или None): {missing:?}"
        );
        let stale: Vec<&str> = UNIQUE_FIELDS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !found.named.contains(*name))
            .collect();
        assert!(stale.is_empty(), "в словаре лишние имена: {stale:?}");
        assert!(found.named.contains("admins_email_key"));
        assert!(found.named.contains("payments_provider_txid_key"));
    }

    #[test]
    fn разбор_находит_именованные_и_безымянные_ограничения() {
        let mut found = Found {
            named: BTreeSet::new(),
            unnamed: Vec::new(),
        };
        scan(
            "CREATE TABLE t (a text UNIQUE, b text, CONSTRAINT t_b_key UNIQUE (b));\n\
             CREATE UNIQUE INDEX IF NOT EXISTS t_lower_key ON t (lower(b));\n\
             -- CONSTRAINT в_комментарии UNIQUE\n\
             ALTER TABLE t ADD CONSTRAINT t_ab_key UNIQUE NULLS NOT DISTINCT (a, b);",
            "test.sql",
            &mut found,
        );
        assert_eq!(
            found.named.into_iter().collect::<Vec<_>>(),
            ["t_ab_key", "t_b_key", "t_lower_key"]
        );
        assert_eq!(found.unnamed, ["test.sql: t a text UNIQUE"]);
    }

    #[test]
    fn имя_поля_берётся_по_имени_ограничения() {
        assert_eq!(field_for("admins_email_key"), Some("Почта"));
        assert_eq!(field_for("jobs_dedupe_key"), None);
        assert!(is_known("jobs_dedupe_key"));
        assert_eq!(field_for("неизвестное"), None);
    }
}
