pub fn default(key: &str) -> Option<&'static str> {
    Some(match key {
        "menu.title" => "<b>{brand}</b>",
        "menu.greeting" => "",
        "menu.no_subscription" => "No subscription yet.",
        "menu.single" => "\n<b>{tariff}</b> · {status}",
        "menu.single_until" => "Until {date}, {days} left",
        "menu.single_ended" => "Ended {date}",
        "menu.single_traffic" => "Traffic: {used} of {limit}",
        "menu.single_traffic_unlimited" => "Traffic: unlimited",
        "menu.single_devices" => "Devices: {count} of {limit}",
        "menu.single_devices_limit" => "Devices allowed: {limit}",
        "menu.balance" => "Balance: {balance}",
        "menu.subscription" => "{tariff}: {status}, until {date}",
        "menu.subscription_open" => "{tariff}: {status}",
        "menu.btn.subscription" => "Subscription",
        "menu.btn.trial" => "Try for free",
        "menu.btn.buy" => "Buy a subscription",
        "menu.btn.extend" => "Renew or change plan",
        "menu.btn.app" => "Open the app",
        "menu.btn.balance" => "Balance",
        "menu.btn.promo" => "Promo code",
        "menu.btn.referral" => "Invite a friend",
        "menu.btn.options" => "Add-ons",
        "menu.btn.contests" => "Giveaways",
        "menu.btn.roulette" => "Lucky spin",
        "menu.btn.gifts" => "Gift a subscription",
        "menu.btn.documents" => "Documents",
        "menu.btn.support" => "Support",
        "menu.btn.email" => "Sign in by email",
        "menu.btn.language" => "Language",
        "lang.choose" => "Choose your language:",
        "status.active" => "active",
        "status.trial" => "free trial",
        "status.limited" => "out of traffic",
        "status.expired" => "expired",
        "status.disabled" => "blocked",
        "status.pending" => "being set up",
        "sub.none" => "No subscription yet.",
        "sub.choose" => "Choose a subscription:",
        "sub.btn.item" => "{tariff} · until {date}",
        "sub.btn.item_open" => "{tariff}",
        "sub.not_found" => "Subscription not found.",
        "sub.title" => "<b>{tariff}</b> · {status}",
        "sub.until" => "Until {date}, {days} left",
        "sub.ended" => "Ended {date}",
        "sub.traffic" => "Traffic: {used} of {limit}{reset}",
        "sub.traffic_reset_date" => ", resets {date}",
        "sub.traffic_reset_payment" => ", resets on renewal",
        "sub.traffic_unlimited" => "Traffic: unlimited",
        "sub.devices" => "Devices: {count} of {limit}",
        "sub.devices_limit" => "Devices allowed: {limit}",
        "sub.autorenew_on" => "Auto-renewal: on",
        "sub.autorenew_off" => "Auto-renewal: off",
        "sub.link" => "\nSubscription link:\n<code>{url}</code>",
        "sub.link_error" => "\nCan't get the link right now. {error}",
        "sub.btn.connect" => "Setup instructions",
        "sub.btn.devices" => "Devices",
        "sub.btn.reissue" => "Get a new link",
        "sub.btn.extend" => "Renew",
        "sub.btn.all" => "All subscriptions",
        "sub.btn.buy" => "Buy a subscription",
        "sub.reissue_confirm" => {
            "The old link will stop working, and so will every device using it. Reconnect the devices you need with the new link."
        }
        "sub.btn.reissue_yes" => "Get a new link",
        "sub.reissued" => "New subscription link:\n<code>{url}</code>\nAdd it to your app again.",
        "sub.renewal_on" => "Auto-renewal: {provider} will charge {amount} around {date}",
        "sub.renewal_cancelling" => "Auto-renewal: turning it off in {provider}",
        "sub.btn.renewal_off" => "Turn off auto-renewal",
        "sub.renewal_confirm" => {
            "Turn off auto-renewal via {provider}? There will be no more charges, the subscription runs until {date}."
        }
        "sub.btn.renewal_off_yes" => "Turn off",
        "sub.renewal_done" => {
            "Auto-renewal via {provider} is off. The subscription runs until {date}."
        }
        "sub.renewal_pending" => {
            "Turning off auto-renewal in {provider}. If it doesn't work right away, the panel will retry by itself."
        }
        "sub.trial_started" => {
            "Your free trial is being set up. It will show up under “Subscription” in a minute."
        }
        "sub.trial_confirm" => {
            "To get the free trial, confirm you're not a robot: tap the button below."
        }
        "sub.btn.trial_confirm" => "I'm not a robot",
        "sub.trial_choose" => "<b>Choose your free trial</b>",
        "sub.trial_variant_line" => "{variant} — {days}, devices: {count}, traffic: {limit}",
        "sub.trial_variant_unlimited" => "{variant} — {days}, devices: {count}, unlimited traffic",
        "sub.btn.trial_variant" => "{variant} · {days}",
        "sub.trial_verify" => {
            "To get the free trial, pass the check that you're not a robot. It opens with the button below, and the trial starts right after it."
        }
        "sub.btn.trial_verify" => "Take the check",
        "sub.trial_verify_off" => {
            "The free trial isn't available right now: the check page doesn't open. Please try later or contact support."
        }
        "dev.empty" => "No devices yet. They appear once you add the subscription to an app.",
        "dev.title" => "<b>Devices</b>",
        "dev.line" => "{n}. {title} — {details}, added {date}",
        "dev.line_plain" => "{n}. {title}, added {date}",
        "dev.btn.disconnect" => "Disconnect {title}",
        "dev.confirm" => {
            "Disconnect {title}? This frees a slot, but if the subscription is still in the app there, it takes the slot again on the next update. To cut off access, get a new link."
        }
        "dev.btn.disconnect_yes" => "Disconnect",
        "dev.disconnected" => "{title} disconnected.",
        "dev.stale" => "The device list is out of date. Open it again.",
        "buy.which" => "Which subscription do you want to renew?",
        "buy.btn.extend_item" => "Renew {tariff} · until {date}",
        "buy.btn.extend_item_open" => "Renew {tariff}",
        "buy.btn.new" => "New subscription",
        "buy.category_renew" => {
            "You already have a subscription in “{category}”: this payment renews it."
        }
        "buy.category_choose" => {
            "You can't add another subscription in “{category}”. Which one do you want to renew?"
        }
        "buy.empty" => "No plans on sale right now.",
        "buy.title" => "<b>Choose a plan</b>",
        "buy.category" => "\n<b>{category}</b>",
        "buy.tariff_line" => "{tariff} — from {per_month} a month",
        "buy.tariff_line_plain" => "{tariff}",
        "buy.tariff_line_promo" => {
            "{tariff} — from <s>{full_per_month}</s> <b>{per_month}</b> a month, −{percent}% · {promotion} {ends}"
        }
        "buy.btn.tariff" => "{tariff}",
        "buy.tariff_title" => "<b>{tariff}</b>",
        "buy.tariff_devices" => "Devices included: {count}",
        "buy.tariff_traffic" => "Traffic: {limit}, {reset}",
        "buy.tariff_unlimited" => "Traffic: unlimited",
        "buy.choose_period" => "\nChoose a period:",
        "buy.btn.period" => "{days} — {price}",
        "buy.btn.period_monthly" => "{days} — {price}, {per_month}/mo",
        "buy.btn.period_savings" => "{days} — {price}, {per_month}/mo, save {savings}%",
        "buy.promotion" => "\n<b>{promotion}</b> {ends}: −{percent}%",
        "buy.promotion_period" => "{days} — <s>{full_price}</s> <b>{price}</b>",
        "buy.btn.period_promo" => "{days} — {price}, −{percent}%",
        "buy.extra_devices" => {
            "The plan includes {included}. Add more? Each one is {price} a month."
        }
        "buy.btn.no_extra" => "No extra devices",
        "buy.btn.extra" => "+{count}",
        "buy.device_discounts" => "Discount on extra devices: {steps}.",
        "buy.summary_title" => "<b>Checkout</b>",
        "buy.summary_line" => "{label} — {amount}",
        "buy.summary_total" => "Total: <b>{total}</b>",
        "buy.summary_total_stars" => "Total: <b>{total}</b> (or {stars})",
        "buy.summary_renewal" => "Renewal: the subscription will run until {date}.",
        "buy.summary_change" => {
            "<b>You're switching to the “{tariff}” plan.</b> Your current subscription ({current}) will be removed. The new one starts right after payment and runs until {date}. Remaining days don't carry over, and your subscription link stays the same."
        }
        "buy.summary_from_trial" => {
            "Your free trial ends right after payment, and “{tariff}” runs until {date}."
        }
        "buy.summary_new" => "The subscription will run until {date}.",
        "buy.btn.method" => "{method} — {amount}",
        "buy.balance_short" => "Balance: {missing} short (you have {balance})",
        "buy.no_methods" => "No payment methods are available right now.",
        "buy.promo_removed" => "{error} Promo code removed.",
        "buy.promo_beaten" => "The promo code is not applied: the “{promotion}” sale gives more.",
        "buy.promotion_beaten" => {
            "The promo code is applied: it gives more than the “{promotion}” sale."
        }
        "buy.btn.change_devices" => "Change extra devices",
        "buy.btn.promo" => "Enter a promo code",
        "buy.btn.promo_remove" => "Remove promo code",
        "buy.ask_promo" => "Enter your promo code:",
        "buy.email_ask" => {
            "{provider} needs an email for this payment: the receipt and payment emails will go there. Send it in one message."
        }
        "buy.email_change_ask" => "Send the new email in one message: receipts will go there.",
        "buy.email_invalid" => {
            "The email seems to have a typo. Send an address like name@example.com."
        }
        "buy.email_line" => "The receipt will go to {email}",
        "buy.btn.email_change" => "Change email",
        "buy.btn.promo_activate" => "Apply promo code",
        "buy.btn.back_to_payment" => "Back to checkout",
        "buy.btn.promo_other" => "Try another",
        "buy.btn.no_promo" => "No promo code",
        "pay.what_tariff" => "{tariff}, {term}.",
        "pay.what_topup" => "Top-up of {amount}.",
        "pay.what_topup_bonus" => {
            "Top-up of {amount}. {total} will be on your balance: {amount} and a {bonus} bonus."
        }
        "pay.ready" => {
            "{what}\nYour invoice is ready: tap “Pay”. We'll confirm here once it's paid."
        }
        "pay.link" => "{what}\nPayment link: {url}",
        "pay.btn.pay" => "Pay",
        "pay.gram_ready" => {
            "{what}\nYour invoice is ready. Pay it in the message with the “Pay in Telegram” button."
        }
        "pay.gram_caption" => {
            "{what}\nAmount due: {amount}. Telegram Wallet opens with the amount and comment filled in. No wallet in Telegram? Tap “Another wallet”."
        }
        "pay.gram_what" => "Subscription.",
        "pay.btn.gram_other" => "Another wallet",
        "pay.balance_done" => {
            "Paid from your balance. The subscription will update within a minute."
        }
        "pay.stars_done" => "Stars payment received. The subscription will update within a minute.",
        "pay.stars_repeat" => {
            "Stars payment received, but this invoice was already paid. We'll check the second payment by hand and get back to you."
        }
        "pay.stars_deferred" => {
            "We got your payment but couldn't process it right away. We'll check it and issue the subscription. If it doesn't appear within an hour, send /paysupport."
        }
        "pay.paysupport" => {
            "Questions about paying with Telegram Stars? Write to support, we reply within a day. If the subscription isn't issued after payment, we'll refund the stars."
        }
        "pay.btn.write_support" => "Contact support",
        "pay.btn.support_bot" => "Support in the bot",
        "bal.amount" => "Balance: <b>{amount}</b>",
        "bal.referral" => "Referral balance: {amount} (withdraw it under “Invite a friend”)",
        "bal.note" => {
            "The balance covers the whole amount at once and can't be combined with a card."
        }
        "bal.off_note" => {
            "Top-ups are unavailable right now. You can still pay for a subscription with what's left if it covers the full amount."
        }
        "bal.btn.preset" => "+{amount}",
        "bal.btn.custom" => "Other amount",
        "bal.bad_amount" => "{error} Enter a different amount.",
        "bal.no_methods" => "Top-ups are unavailable: no payment system is connected.",
        "bal.choose_method" => "Top-up of {amount}. Choose a payment method:",
        "bal.btn.method" => "{method}",
        "bal.btn.method_hint" => "{method} · {hint}",
        "bal.enter_amount" => "Enter the top-up amount, {example}.",
        "bal.not_understood" => "Couldn't read the amount. Enter a number, for example 10.",
        "sup.prompt" => "Describe your question in one message. We'll reply here in the bot.",
        "sup.btn.other" => "Other ways to reach us",
        "sup.link" => "Write to us: the button below opens the support chat.",
        "sup.btn.write" => "Contact support",
        "sup.sent" => "Request #{id} sent. We'll reply here in the bot.",
        "sup.added" => "Message added to request #{id}. We'll reply here in the bot.",
        "sup.reply_prompt" => "Write your reply in one message. It will go to request #{id}.",
        "sup.file_big" => {
            "The file is larger than {mb} MB, so we can't accept it. Please send a smaller file."
        }
        "sup.file_type" => {
            "We can't accept this file. Photos in JPG, PNG, WebP or GIF and PDF, TXT or LOG files will do."
        }
        "sup.file_off" => {
            "Files aren't accepted in requests right now. Please describe your question in text."
        }
        "sup.file_failed" => "Couldn't get the file. Please send it again.",
        "promo.prompt" => {
            "Enter a promo code. Discounts apply at checkout, free days are added to your subscription right away."
        }
        "promo.saved" => "{code} saved: the discount applies at checkout.",
        "promo.btn.choose_tariff" => "Choose a plan",
        "promo.which_sub" => "Which subscription should get the days?",
        "promo.days_added" => "Added {days} to your subscription.",
        "ref.title" => "<b>Invite a friend</b>",
        "ref.percent" => "Share your link: you get {percent}% of every payment your friends make.",
        "ref.link" => "<code>{link}</code>",
        "ref.invited" => "\nInvited: {count}",
        "ref.earned" => "Earned: {amount}",
        "ref.balance" => "Referral balance: {amount}",
        "ref.withdrawal" => "Request #{id} for {amount}: {status}",
        "ref.btn.cancel_withdrawal" => "Cancel request #{id}",
        "ref.btn.withdraw" => "Withdraw money",
        "ref.btn.share" => "Share the link",
        "ref.how" => "How would you like to get the money?",
        "ref.amount" => "How much to withdraw? Your referral balance is {amount}.{minimum}",
        "ref.minimum" => " Minimum: {amount}.",
        "ref.cancelled" => "Request #{id} cancelled, {amount} is back on your referral balance.",
        "ref.details" => "Enter your payout details. {hint}",
        "ref.requested" => {
            "Request #{id} for {amount} accepted. The amount is taken from your referral balance and comes back if the request is declined."
        }
        "ref.btn.retry" => "Try again",
        "contest.list" => "<b>Giveaways</b>\nChoose one:",
        "contest.none" => "No giveaways right now. The bot will message you when a new one starts.",
        "contest.title" => "<b>{title}</b>",
        "contest.prize" => "Prize: {prize}. Winners: {winners}.",
        "contest.prizes" => "Prizes:",
        "contest.prize_line" => "{places} — {prize}",
        "contest.winners" => "Winners: {winners}.",
        "contest.until" => "Results on {date}.",
        "contest.conditions" => "\n<b>Requirements</b>",
        "contest.condition_ok" => "✅ {condition}",
        "contest.condition" => "• {condition}",
        "contest.condition_no" => "▫️ {condition}",
        "contest.open" => "Anyone can take part.",
        "contest.entries" => "\nParticipants: {count}",
        "contest.joined" => "\nYou're in. The results will come here, to the bot.",
        "contest.joined_now" => "Done, you're in the “{title}” giveaway. Results on {date}.",
        "contest.not_eligible" => {
            "\nTo take part, meet the requirements marked ▫️ and tap “Join” again."
        }
        "contest.closed" => "Entries for this giveaway are closed.",
        "contest.fair" => "\nDraw fingerprint: <code>{hash}</code>",
        "contest.drawing" => "\nEntries are closed, the bot is drawing the winners.",
        "contest.won" => "\nYou won! Prize: {prize}. {result}",
        "contest.won_place" => "\nYou took {place} place! Prize: {prize}. {result}",
        "contest.won_code" => "Your promo code: <code>{code}</code>. Enter it at checkout.",
        "contest.lost" => {
            "\nThe results are in. This time the prize went to others. Thanks for taking part!"
        }
        "contest.excluded" => "\nThe results are in. You weren't included in the draw: {comment}.",
        "contest.btn.join" => "Join",
        "contest.btn.open" => "{title}",
        "contest.btn.buy" => "Buy a subscription",
        "contest.btn.invite" => "Invite a friend",
        "contest.btn.all" => "All giveaways",
        "contest.confirm" => {
            "To join the giveaway, confirm you're not a robot: tap the button below."
        }
        "contest.denied" => {
            "Giveaways aren't available for this account. If this is a mistake, please contact support."
        }
        "contest.btn.confirm" => "I'm not a robot",
        "opt.no_subscription" => "You can add devices or traffic to an active subscription.",
        "opt.which_sub" => "Which subscription is it for?",
        "opt.menu" => "The subscription runs until {date}. What would you like to add?",
        "opt.btn.devices" => "Devices",
        "opt.btn.traffic" => "Traffic",
        "opt.max" => "No more devices can be added to this subscription.",
        "opt.offer" => {
            "Devices: {included} in the plan, {extra} added, up to {can_add} more.\n{days} left on the subscription. One device for that time costs {price}{stars}."
        }
        "opt.discounts" => {
            "Discount on extra devices: {steps}. The step depends on how many extra devices the subscription has after the purchase."
        }
        "opt.stars" => " or {stars}",
        "opt.btn.count" => "+{count}",
        "opt.traffic" => {
            "Used {used} of {limit}. Extra traffic is used after the main allowance.\n{note}\nChoose a pack:"
        }
        "opt.traffic_burn_payment" => "Whatever you don't use before renewal expires.",
        "opt.traffic_carry_payment" => "Unused traffic carries over after renewal.",
        "opt.traffic_burn_reset" => "Whatever you don't use before the traffic reset expires.",
        "opt.traffic_carry_reset" => {
            "Unused traffic carries over to the next period after the reset."
        }
        "opt.btn.pack" => "+{gb} GB · {price}",
        "opt.quote" => "{label} — {total}{stars}.\nHow would you like to pay?",
        "gift.intro" => {
            "<b>Gift a subscription</b>\nPick a plan and a period, pay, and get a code. Your friend activates it in this bot or in their account on the website."
        }
        "gift.btn.buy" => "Choose a gift",
        "gift.btn.mine" => "My gifts",
        "gift.btn.redeem" => "I have a code",
        "gift.choose_tariff" => "Which subscription would you like to gift?",
        "gift.no_tariffs" => "Gifting isn't available right now: no plans can be gifted.",
        "gift.choose_period" => "“{tariff}” subscription. For how long?",
        "gift.btn.period" => "{term} · {price}",
        "gift.btn.period_promo" => "{term} · {price}, −{percent}%",
        "gift.quote" => {
            "{label} — {total}{stars}.\nThe code for your friend will arrive here right after payment. How would you like to pay?"
        }
        "gift.stars" => " or {stars}",
        "gift.mine_title" => "<b>Your gifts</b>",
        "gift.mine_empty" => "You haven't gifted anything yet.",
        "gift.mine_item" => "<code>{code}</code> — “{tariff}” for {term}\n{status}",
        "gift.mine_link" => "Link for your friend: {link}",
        "gift.state_active" => "Waiting to be activated until {date}",
        "gift.state_active_open" => "Waiting to be activated",
        "gift.state_redeemed" => "Activated {date}",
        "gift.state_expired" => "Code expired {date}",
        "gift.state_canceled" => "Cancelled, money refunded",
        "gift.ask_code" => "Send the gift code in one message. It looks like this: ABCD-EFGH-JKLM.",
        "gift.preview" => "<b>Gift:</b> “{tariff}” subscription for {term}.",
        "gift.plan_new" => "The subscription starts working right after activation.",
        "gift.plan_separate" => {
            "This will be a separate subscription with its own link. Your current ones stay as they are."
        }
        "gift.plan_renewal" => "The time will be added to your “{current}” subscription.",
        "gift.plan_change" => {
            "You'll switch to the “{tariff}” plan. Your current subscription ({current}) will be replaced, remaining days don't carry over."
        }
        "gift.plan_trial" => "Your free trial ends and “{tariff}” starts working right away.",
        "gift.own" => {
            "This is your own gift: once you activate it, the code won't work for a friend."
        }
        "gift.choose_target" => "Renew one of your subscriptions or start a new one?",
        "gift.plan_limit" => {
            "You can't add another subscription in this category, so the time will be added to one of yours."
        }
        "gift.choose_existing" => "Which subscription do you want to renew?",
        "gift.btn.activate" => "Activate",
        "gift.btn.extend" => "Renew “{tariff}”",
        "gift.btn.new" => "New subscription",
        "gift.paid_balance" => {
            "Paid from your balance. The gift code will arrive here in a minute."
        }
        "gift.redeemed" => {
            "Gift activated: “{tariff}” for {term}. The subscription will be ready in a minute, we'll message you."
        }
        "pl.done" => {
            "A subscription for {days} is set up. We'll message you as soon as it's working."
        }
        "pl.not_found" => "This promo link doesn't exist. Check the link.",
        "pl.disabled" => "This promo link no longer works.",
        "pl.expired" => "This promo link has expired.",
        "pl.exhausted" => {
            "This link has run out: it has already been used as many times as allowed."
        }
        "pl.already_used" => {
            "You've already got a subscription with this link. Each link works once per person."
        }
        "pl.has_subscription" => {
            "You already have an active subscription, and this link gives a new one. You can use it once your subscription ends."
        }
        "pl.confirm" => {
            "To get the subscription from this promo link, confirm you're not a robot: tap the button below."
        }
        "pl.btn.confirm" => "I'm not a robot",
        "pl.denied" => {
            "Promo link subscriptions aren't available for this account. If this is a mistake, please contact support."
        }
        "pl.again" => "Open the promo link again to get the subscription.",
        "pay.what_devices" => "extra devices: +{count}",
        "pay.what_gift" => "gift: “{tariff}” for {term}",
        "pay.what_traffic" => "traffic: +{gb} GB",
        "email.title" => "<b>Sign in by email</b>",
        "email.none" => {
            "No email is linked. Link one to sign in to your account on the website when Telegram doesn't work."
        }
        "email.current" => "Email: {email}",
        "email.unconfirmed" => {
            "The email isn't confirmed. Confirm it with a code from an email to be able to restore the password."
        }
        "email.password_yes" => "Website password is set.",
        "email.password_no" => "Website password is not set.",
        "email.how_password" => {
            "If Telegram doesn't work, open {url}, choose “Email” and sign in with {email} and your password. Forgot it? Tap “Forgot your password?” and a code for a new password will come by email."
        }
        "email.how_no_letters" => {
            "If Telegram doesn't work, open {url}, choose “Email” and sign in with {email} and your password. Forgot it? Contact support."
        }
        "email.how_code" => {
            "If Telegram doesn't work, open {url}, choose “Email”, enter {email} and tap “Send code”: a sign-in code will come by email."
        }
        "email.ask" => {
            "Send your email address in one message. A confirmation code will be sent to it."
        }
        "email.ask_no_code" => {
            "Send your email address in one message. There will be no confirmation code, so check the address."
        }
        "email.invalid" => {
            "The address looks wrong. Send it in full, for example name@example.com."
        }
        "email.ask_app_code" => {
            "Your account is protected with an app code. To link {email}, send the six-digit code from your authenticator app."
        }
        "email.ask_password" => {
            "To change your email to {email}, send your current website password. The bot deletes the message with the password right away, but its text may stay in phone notifications."
        }
        "email.change_in_app" => {
            "Changing the email needs your website password. Enter it in the app so it doesn't stay in the chat."
        }
        "email.code_sent" => {
            "The code was sent to {email}. Send it here, it's valid for {minutes}."
        }
        "email.code_again" => "Send the code from the email to {email} or ask for a new one.",
        "email.linked" => "The email {email} is linked.",
        "email.linked_no_code" => {
            "The email {email} is linked without a check because emails aren't being sent. You can sign in to the website with it only using a password."
        }
        "email.confirmed" => "The email {email} is confirmed.",
        "email.set_password_app" => {
            "Now set a website password: the button below opens it in the app."
        }
        "email.set_password_chat" => {
            "Now set a website password: send it in the next message, at least 10 characters. The bot deletes the message with the password right away, but its text may stay in phone notifications. If you'd rather not send it in the chat, set it in your account on the website: {url}"
        }
        "email.password_saved" => "The website password is set.",
        "email.secret_left" => {
            "Your message couldn't be deleted from the chat. Please delete it yourself."
        }
        "email.off" => {
            "An email can't be linked right now: website sign-in is off or emails aren't being sent. If you need help, contact support."
        }
        "email.error" => "{error}",
        "email.btn.link" => "Link email",
        "email.btn.confirm" => "Confirm email",
        "email.btn.change" => "Change email",
        "email.btn.change_app" => "Change email in the app",
        "email.btn.password" => "Set password",
        "email.btn.resend" => "Send the code again",
        "email.btn.other" => "Use another email",
        "login.title" => "<b>Account sign-in</b>",
        "login.account" => "Account: {who}",
        "login.ip" => "From: {ip}",
        "login.browser" => "Browser: {agent}",
        "login.hint" => "If this was you, confirm. If not, tap “It wasn't me”.",
        "login.btn.confirm" => "Confirm sign-in",
        "login.btn.reject" => "It wasn't me",
        "login.confirmed" => "Sign-in confirmed. Go back to your account.",
        "login.rejected" => "Sign-in declined. If it wasn't you, there's nothing else to do.",
        "login.expired" => {
            "This sign-in request has expired. Start signing in on the website again."
        }
        "login.error" => "Account sign-in: {error}",
        "login.limit" => {
            "You need an account to sign in to the website, and a new one can't be created from your address right now. Send /start to use the bot, or try signing in to the website later."
        }
        "login.busy" => {
            "Too many new accounts are being created on the website right now. Please try signing in again in a minute, or send /start to use the bot."
        }
        "link.title" => "<b>Linking Telegram</b>",
        "link.account" => "Account on {site}: {account}",
        "link.who" => "Your Telegram: {who}",
        "link.empty" => {
            "This Telegram has its own account in the bot, but it is empty: no subscriptions or payments. After linking, this Telegram moves to the account on the website, and the empty bot account will be closed."
        }
        "link.hint" => {
            "This is not a website sign-in. After linking, the bot and the app open this account, and your Telegram can be used to sign in to it on the website. If you didn't start this, tap “Cancel”."
        }
        "link.btn.confirm" => "Link",
        "link.btn.cancel" => "Cancel",
        "link.done" => {
            "Telegram is linked. Your subscription and payments are now available here in the bot too."
        }
        "link.same" => "This Telegram is already linked to your account.",
        "link.canceled" => "Linking cancelled, nothing has changed.",
        "link.error" => "Linking Telegram: {error}",
        "docs.intro" => "Before you start, please read these documents:",
        "docs.item" => "• <a href=\"{url}\">{title}</a>",
        "docs.agree" => "\nBy tapping “I accept”, you agree to them.",
        "docs.btn.accept" => "I accept",
        "docs.list_title" => "<b>Documents</b>",
        "notify.payment_paid" => {
            "Payment of {amount} received. The subscription will update within a minute."
        }
        "notify.payment_paid_plain" => {
            "Payment received. The subscription will update within a minute."
        }
        "notify.topup_done" => "Your balance is topped up by {amount}.",
        "notify.topup_done_bonus" => "{total} is on your balance: {amount} and a {bonus} bonus.",
        "notify.topup_done_plain" => "Your balance is topped up.",
        "notify.subscription_ready" => {
            "Your subscription is ready and runs until {date}. The link is under “Subscription”."
        }
        "notify.subscription_ready_plain" => {
            "Your subscription is ready. The link is under “Subscription”."
        }
        "notify.subscription_renewed" => "Subscription renewed until {date}.",
        "notify.subscription_renewed_plain" => "Subscription renewed.",
        "notify.subscription_expiring" => {
            "Your subscription ends on {date}. Renew it to keep the VPN on."
        }
        "notify.subscription_expiring_plain" => {
            "Your subscription ends soon. Renew it to keep the VPN on."
        }
        "notify.autorenew_upcoming" => {
            "On {date} we'll charge {amount} to renew your subscription. If your balance is short, top it up in advance."
        }
        "notify.autorenew_upcoming_plain" => {
            "We'll renew your subscription soon. If your balance is short, top it up in advance."
        }
        "notify.autorenew_failed" => {
            "Couldn't renew your subscription: {missing} short. Top up your balance and we'll renew it automatically."
        }
        "notify.autorenew_failed_plain" => {
            "Couldn't renew your subscription. Top up your balance and we'll renew it automatically."
        }
        "notify.autorenew_pay" => {
            "Automatic renewal didn't go through. Renew by {date} to keep the VPN on."
        }
        "notify.autorenew_pay_plain" => {
            "Automatic renewal didn't go through. Renew by hand to keep the VPN on."
        }
        "notify.autorenew_disabled" => {
            "Auto-renewal is off because the payment failed. Renew the subscription by hand."
        }
        "notify.renewal_cancelled" => {
            "Auto-renewal via {provider} is off, there will be no more charges. Your subscription runs until {date}."
        }
        "notify.renewal_cancelled_plain" => {
            "Auto-renewal via {provider} is off, there will be no more charges."
        }
        "notify.autorenew_after_off" => {
            "The {amount} renewal charge went through before auto-renewal was turned off, so your subscription is renewed until {date}. If you don't need it, contact support and we'll refund you."
        }
        "notify.autorenew_after_off_plain" => {
            "The renewal charge went through before auto-renewal was turned off, so your subscription is renewed. If you don't need it, contact support and we'll refund you."
        }
        "notify.payment_refunded_blocked" => {
            "Payment of {amount} refunded: the account is blocked and the order wasn't delivered."
        }
        "notify.payment_refunded_blocked_plain" => {
            "Payment refunded: the account is blocked and the order wasn't delivered."
        }
        "notify.stars_refunded" => "Stars for “{label}” refunded: {stars}.",
        "notify.stars_refunded_plain" => "Stars refunded: {stars}.",
        "notify.support_reply" => "<b>Support reply</b>\n\n{body}",
        "notify.gift_ready" => "Gift paid: “{tariff}” for {term}.\nCode: <code>{code}</code>",
        "notify.gift_ready_link" => {
            "Send your friend this link, the gift activates in the bot:\n{link}"
        }
        "notify.gift_ready_cabinet" => "Or on the website: {url}",
        "notify.gift_ready_until" => "The code is valid until {date}.",
        "notify.gift_redeemed" => {
            "Your gift <code>{code}</code> was activated: “{tariff}” for {term}."
        }
        "notify.btn.gifts" => "My gifts",
        "notify.oauth_linked" => {
            "Sign-in with {provider} is now linked to your account: {account}. If this wasn't you, contact support right away."
        }
        "notify.oauth_unlinked" => {
            "You confirmed your email with a code, so we unlinked the Google or Apple sign-in that was added before the confirmation and signed out all website sessions. If you added it yourself, link it again under Security."
        }
        "notify.email_set" => {
            "An administrator set the email {email} on your account. If you didn't ask for this, contact support right away."
        }
        "notify.email_removed" => {
            "An administrator removed the email {email} from your account, so you can no longer sign in to the website with it. If you didn't ask for this, contact support right away."
        }
        "notify.password_issued" => {
            "An administrator issued you a temporary password for the website. Set your own password when you first sign in with it. If you didn't ask for a new password, contact support right away."
        }
        "notify.referral_credit" => "You earned {amount} from a friend's payment.",
        "notify.withdrawal_paid" => "Your withdrawal of {amount} has been paid.",
        "notify.withdrawal_rejected" => {
            "Your withdrawal of {amount} was declined: {comment}. The money is back on your referral balance."
        }
        "notify.contest_won" => "You won the “{title}” giveaway!\nPrize: {prize}. {result}",
        "notify.contest_won_code" => "Your promo code: <code>{code}</code>. Enter it at checkout.",
        "notify.contest_waiting" => {
            "You won the “{title}” giveaway!\nPrize: {prize}. The days will be added as soon as you have a subscription."
        }
        "notify.contest_won_place" => {
            "You took {place} place in the “{title}” giveaway!\nPrize: {prize}. {result}"
        }
        "notify.contest_waiting_place" => {
            "You took {place} place in the “{title}” giveaway!\nPrize: {prize}. The days will be added as soon as you have a subscription."
        }
        "notify.withdrawal_rejected_plain" => {
            "Your withdrawal of {amount} was declined. The money is back on your referral balance."
        }
        "notify.traffic_warning" => {
            "{when} you've used {used}, over the {limit} {per} limit. Please ease off, or access may be restricted for a while."
        }
        "notify.traffic_restricted" => {
            "{when} you've used {used} with a limit of {limit} {per}. Access is restricted until {until} and comes back on its own."
        }
        "notify.traffic_lifted" => "The traffic restriction is lifted, the VPN works again.",
        "notify.btn.subscription" => "Subscription",
        "notify.btn.balance" => "Balance",
        "notify.btn.extend" => "Renew",
        "notify.btn.ticket" => "Open conversation",
        "notify.btn.reply" => "Reply",
        "notify.btn.support" => "Support",
        "notify.btn.referral" => "Invite a friend",
        "notify.btn.contests" => "Giveaways",
        "notify.btn.email" => "Sign in by email",
        "common.btn.menu" => "Main menu",
        "common.btn.back" => "Back",
        "common.btn.cancel" => "Cancel",
        "common.btn.subscription" => "Subscription",
        "common.blocked" => "Your account is blocked. Please contact support.",
        "common.flood" => {
            "Too many messages in a row. The bot will stop answering for a little while, please try again later."
        }
        "common.busy" => {
            "Too many new users are coming to the bot right now. Please try again in a minute."
        }
        _ => return None,
    })
}
