pub fn by_code(code: &str) -> Option<&'static str> {
    Some(match code {
        "not_found" | "remnawave.not_found" => "Not found.",
        "auth.required" => "Please sign in again.",
        "system.restoring" => {
            "The service is being restored from a backup. Please try again in a few minutes."
        }
        "auth.forbidden" => "Access denied.",
        "auth.rate_limited" => "Too many attempts. Please wait a little and try again.",
        "auth.app_code_required" => "Enter the code from your authenticator app.",
        "auth.code_attempts" => "Too many wrong codes. Request a new code and try again.",
        "auth.code_invalid" => "The code is wrong or has expired.",
        "auth.email_code_mode" => "Email sign-in works with a code from the email.",
        "auth.email_password_mode" => "Email sign-in works with a password, no code needed.",
        "auth.email_taken" => "This email is already registered. Sign in with your password.",
        "auth.email_confirm" => "Email sign-up needs a code from the email.",
        "auth.signup_closed" => {
            "Email sign-up is unavailable right now because emails with codes aren't being sent. Sign in with Telegram or contact support."
        }
        "auth.signup_expired" => {
            "The time to confirm your email ran out or sign-up was started again. Please sign up once more."
        }
        "auth.email_change_off" => {
            "Emails aren't being sent right now, so the email can't be changed."
        }
        "auth.email_code_needed" => {
            "The email is linked with a code from an email. Open this section again and try once more."
        }
        "auth.email_unconfirmed" => {
            "This email is on an account without confirmation, so a code from an email doesn't open it. If the account is yours, sign in with Telegram and confirm the email: Profile → Email in the mini app, or Security → Email in the account. If it isn't, please contact support."
        }
        "auth.email_in_use" => "This email already belongs to another account.",
        "auth.email_same" => "This email is already on your account.",
        "auth.init_data" => {
            "Couldn't sign you in: the data from Telegram didn't match. Close the mini app and open it again from the bot. If that doesn't help, please contact support."
        }
        "auth.init_data_old" => {
            "Couldn't sign you in: the data from Telegram is out of date. Close the mini app and open it again from the bot. If that doesn't help, please contact support."
        }
        "auth.login_expired" => "The sign-in has expired. Please start again.",
        "auth.miniapp_off" => "The mini app is turned off. Please use the bot.",
        "auth.no_email" => {
            "No email is linked to this account, so a website password isn't needed."
        }
        "auth.reset_unavailable" => "The password can't be reset by email. Please contact support.",
        "auth.password_invalid" => "The website password is wrong.",
        "auth.password_required" => "Enter your website password.",
        "auth.same_password" => "The new password is the same as the old one.",
        "auth.server_clock" => {
            "We can't sign you in right now: the time on our server is wrong. The problem is on our side. Please try again later or contact support."
        }
        "auth.site_off" => "Website sign-in is turned off. Please use the bot.",
        "auth.totp_code" => "The code didn't work. Check the time on your phone.",
        "auth.totp_disabled" => "Two-step sign-in isn't turned on.",
        "auth.totp_enabled" => "Two-step sign-in is already on.",
        "auth.totp_secret" => "The setup key is damaged. Start the setup again.",
        "auth.totp_required" => "Enter the code from your authenticator app.",
        "auth.totp_invalid" => "The code didn't work. Check the time on your phone.",
        "auth.weak_password" => "The password must be 10 to 256 characters long.",
        "auth.wrong_password" => "Wrong email or password.",
        "oauth.failed" => "Sign-in didn't go through. Please try again.",
        "oauth.canceled" => "Sign-in was cancelled.",
        "oauth.expired" => "The sign-in has expired. Please start again.",
        "oauth.refused" => "The sign-in wasn't confirmed. Please try again.",
        "oauth.unavailable" => {
            "The sign-in service isn't responding. Please try again a bit later."
        }
        "oauth.bad_token" | "oauth.bad_reply" => {
            "Couldn't verify the sign-in reply. Please start signing in again."
        }
        "oauth.blocked" => "Your account is blocked. If this is a mistake, please contact support.",
        "oauth.email_taken" => {
            "This email already belongs to another account. Sign in to it the usual way and link this sign-in under Security."
        }
        "oauth.other_account" => {
            "Another account of this kind is already linked to the account with this email. Sign in with it or the usual way."
        }
        "oauth.taken" => {
            "This account is already used to sign in elsewhere. Unlink it there first."
        }
        "oauth.already_linked" => {
            "This sign-in is already linked. To switch accounts, unlink the old one first."
        }
        "oauth.session_gone" => "Your session has ended. Sign in again and repeat the linking.",
        "oauth.not_configured" => "This sign-in method isn't available right now.",
        "oauth.no_request" => "Sign-in didn't finish. Tap the sign-in button again.",
        "oauth.not_linked" => "This sign-in method isn't linked.",
        "oauth.last_method" => {
            "This is your only sign-in method. Link another one first: Telegram, email or another account."
        }
        "telegram_link.expired" => {
            "This link has expired. Open Security on the website and start again."
        }
        "telegram_link.taken" => {
            "This Telegram already has its own account. Sign in to it or contact support."
        }
        "telegram_link.already_linked" => "A Telegram is already linked to this account.",
        "telegram_link.unavailable" => "Linking Telegram isn't available right now.",
        "telegram_link.session_gone" => {
            "Your website session has ended. Sign in again and repeat the linking."
        }
        "telegram_link.blocked" => {
            "Your account is blocked. If this is a mistake, please contact support."
        }
        "telegram_link.email_unverified" => {
            "Confirm your email first: sign in with a code from the letter or reset your password with “Forgot password”. Then link Telegram."
        }
        "telegram_link.email_unconfirmable" => {
            "Your email has to be confirmed first, but letters aren't being sent right now. Please contact support."
        }
        "app_link.expired" => "The link has expired. Go back to Telegram and tap the button again.",
        "app_link.taken" => {
            "This link is already open in another window. Go back to Telegram and tap the button again."
        }
        "app_link.no_app" => {
            "This button is gone. Go back to Telegram and open the connection page again."
        }
        "app_link.not_ready" => {
            "Your subscription isn't ready yet. The link will appear once it starts working."
        }
        "antibot.expired" => "The check has expired. Request the free trial again.",
        "antibot.too_fast" => "Tap the button once more.",
        "antibot.confirm" => "First confirm you're not a robot.",
        "antibot.failed" => "The check didn't pass. Please try again.",
        "antibot.unavailable" => {
            "We couldn't check that you're not a robot: the check service didn't answer. Please try again in a minute."
        }
        "antibot.captcha" => "Pass the check that you're not a robot.",
        "signup.limit" => {
            "A new account can't be created from your address right now. Open the bot or try again later."
        }
        "signup.busy" => {
            "Too many new accounts are being created right now. Please try again in a minute."
        }
        "antibot.denied" => {
            "The free trial isn't available for this account. If this is a mistake, please contact support."
        }
        "balance.insufficient" => "Not enough money on your balance.",
        "balance.topup_out_of_range" => "This top-up amount isn't allowed.",
        "balance.currency" => "The balance is kept in a different currency.",
        "checkout.balance_currency" => {
            "Only amounts in the shop currency can be paid from the balance."
        }
        "checkout.method_unavailable" => "This payment method isn't available here.",
        "checkout.topup_internal" => "Top-ups are paid through a payment system only.",
        "client.action_off" => "You can't do this yourself. Please contact support.",
        "client.locale" => "This language is turned off right now.",
        "config.public_url" => "Payments aren't set up yet. Please contact support.",
        "gram.closed" => "This invoice is already closed. Please start the payment again.",
        "module.disabled" => "This feature is turned off right now.",
        "module.unavailable" => "This feature is unavailable right now.",
        "not_implemented" => "This feature isn't available yet. Try later or contact support.",
        "order.awaiting_payment" => "The payment hasn't arrived yet. Please wait a little.",
        "order.delivery_running" => "This order is already being delivered. Please wait.",
        "payment.email_required" => "This payment method needs an email. Choose another one.",
        "payment.error" => {
            "The payment system didn't respond. Try another method or try again later."
        }
        "promo.disabled" => "Promo codes aren't accepted right now.",
        "remnawave.not_configured" | "remnawave.unavailable" | "remnawave.error" => {
            "The VPN service is unavailable right now. Try again in a few minutes."
        }
        "roulette.blocked" => "Your account is blocked.",
        "roulette.empty" => "No prizes left for today. Come back later.",
        "roulette.no_spins" => "No spins yet.",
        "roulette.not_ready" => "The lucky spin isn't set up yet. Come back later.",
        "stars.foreign_invoice" => "This invoice belongs to another account.",
        "stars.invoice_mismatch" => "The invoice has expired. Please start again.",
        "stars.unknown_invoice" => "Invoice not found.",
        "subscription.disabled" => {
            "The subscription is blocked, so payment isn't available. Please contact support."
        }
        "subscription.ended" => "The subscription has ended.",
        "subscription.category_full" => {
            "You can't add another subscription in this category. Renew the one you have or choose a plan from another category."
        }
        "subscription.choose" => {
            "Choose which subscription in this category to renew: a new one can't be added."
        }
        "support.body" => "The message is empty or too long.",
        "support.closed" => "This request is closed. Please open a new one.",
        "support.too_many" => "You have too many open requests. Please wait for a reply.",
        "support.files_quota" => {
            "There's no room for more files. Please describe the issue in text."
        }
        "tariff.period_not_found" => "This period isn't available.",
        "tariff.period_inactive" => "This period isn't on sale.",
        "tariff.not_on_sale" => "This plan isn't on sale.",
        "tariff.no_price" => "This period has no price in the shop currency.",
        "tariff.no_stars_price" => "This period can't be paid with stars.",
        "tariff.extra_devices_not_sold" => "Extra devices aren't sold for this plan.",
        "tariff.too_many_devices" => "That's more extra devices than allowed.",
        "topup.currency" => "The balance is topped up in a different currency.",
        "trial.disabled" => "The free trial isn't available right now.",
        "trial.has_subscription" => {
            "The free trial is only available before your first subscription."
        }
        "trial.no_identity" => "The free trial needs an account with Telegram or email.",
        "trial.not_configured" => "The free trial isn't set up yet.",
        "trial.used" => "You've already used the free trial.",
        "trial.variant_required" => "Choose which free trial to start.",
        "trial.variant_unavailable" => {
            "This free trial option isn't available right now. Please choose another one."
        }
        "addon.no_tariff" => "This subscription has no plan.",
        "addon.unlimited" => "This subscription has unlimited traffic, nothing to add.",
        "addon.wait" => "The previous order is still being delivered. Please wait a minute.",
        "mail.address" => "We couldn't send the email to this address. Please check it.",
        "mail.unavailable" | "mail.failed" => "We couldn't send the email. Please try again later.",
        "referral.withdrawals_off" => "Withdrawals aren't accepted right now.",
        "referral.method_unavailable" => {
            "This withdrawal method isn't available right now. Choose another one."
        }
        "referral.bad_amount" => "The amount looks wrong.",
        "referral.below_minimum" => "The amount is below the minimum.",
        "referral.not_enough" => "There isn't enough on your referral balance.",
        "referral.bad_details" => "The payout details look wrong.",
        "referral.already_decided" => "This request has already been processed.",
        "validation.invalid_value" | "validation.unknown_variant" => "Some data looks wrong.",
        "internal" => "Something went wrong. Please try again.",
        _ => return None,
    })
}

pub fn message(code: &str) -> Option<String> {
    by_code(code).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn английские_ошибки_без_кириллицы_и_с_точкой() {
        for code in [
            "auth.rate_limited",
            "auth.init_data",
            "auth.init_data_old",
            "auth.server_clock",
            "app_link.expired",
            "app_link.taken",
            "app_link.no_app",
            "app_link.not_ready",
            "trial.used",
            "trial.variant_required",
            "trial.variant_unavailable",
            "antibot.expired",
            "antibot.too_fast",
            "antibot.confirm",
            "antibot.failed",
            "antibot.unavailable",
            "antibot.captcha",
            "antibot.denied",
            "signup.limit",
            "signup.busy",
            "payment.error",
            "support.closed",
            "support.files_quota",
            "remnawave.error",
        ] {
            let text = by_code(code).unwrap();
            assert!(text.ends_with('.'), "{code}");
            assert!(!text.chars().any(|c| ('а'..='я').contains(&c)), "{code}");
        }
        assert_eq!(by_code("admins.exists"), None);
    }
}
