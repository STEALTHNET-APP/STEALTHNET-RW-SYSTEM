use sn_domain::client::Locale;
use sn_domain::lang;

pub fn text(russian: &str) -> Option<&'static str> {
    Some(match russian {
        "BTC в сети Bitcoin" => "BTC on the Bitcoin network",
        "Gram, бывший Toncoin, в сети TON" => "Gram (formerly Toncoin) on TON",
        "LTC в сети Litecoin" => "LTC on the Litecoin network",
        "USDT в сети BNB Smart Chain" => "USDT on BNB Smart Chain",
        "USDT в сети TON" => "USDT on TON",
        "USDT в сети Tron" => "USDT on Tron",
        "USDT и другие монеты" => "USDT and other coins",
        "Visa и Mastercard любых стран" => "Visa and Mastercard from any country",
        "Банковская карта" => "Bank card",
        "Банковская карта через ЮMoney" => "Bank card via YooMoney",
        "Все способы" => "All methods",
        "ЕРИП" => "ERIP",
        "Звёзды Telegram" => "Telegram Stars",
        "Из кошелька ЮMoney" => "From a YooMoney wallet",
        "Иностранная карта" => "Foreign card",
        "Карта" => "Card",
        "Карта (Беларусь)" => "Card (Belarus)",
        "Карта или СБП" => "Card or SBP",
        "Карта с автопродлением" => "Card with auto-renewal",
        "Карты, СБП, SberPay и T-Pay" => "Cards, SBP, SberPay and T-Pay",
        "Карты белорусских банков" => "Belarusian bank cards",
        "Карты зарубежных банков" => "Cards from banks outside Russia",
        "Карты российских банков" => "Russian bank cards",
        "Карты российских банков и СБП" => "Russian bank cards and SBP",
        "Кошелёк ЮMoney" => "YooMoney wallet",
        "Криптовалюта" => "Crypto",
        "Любая монета" => "Any coin",
        "Монету и сеть выбирают на странице Cryptomus" => {
            "Pick the coin and network on the Cryptomus page"
        }
        "Монету и сеть выбирают на странице Heleket" => {
            "Pick the coin and network on the Heleket page"
        }
        "Оплата в Telegram" => "Pay in Telegram",
        "Перевод с криптокошелька" => "Transfer from a crypto wallet",
        "С баланса" => "From balance",
        "СБП" => "SBP",
        "Со счёта PayPal" => "From a PayPal account",
        "Списывается вся сумма сразу" => {
            "The whole amount is charged at once"
        }
        "Способ выбирается на странице оплаты" => {
            "Choose the method on the payment page"
        }
        "Только у сроков с подпиской Lava.top" => {
            "Only for terms with a Lava.top subscription"
        }
        "Через СберБанк Онлайн" => "Via SberBank Online",
        "Через ЮMoney" => "Via YooMoney",
        "Через интернет-банк" => "Via online banking",
        "Через приложение Т-Банка" => "Via the T-Bank app",
        "Через приложение банка" => "Via your bank's app",
        "ЮMoney" => "YooMoney",
        "каждый месяц" => "every month",
        "криптовалюта TON" => "TON crypto",
        "раз в 180 дней" => "every 180 days",
        "раз в 90 дней" => "every 90 days",
        "раз в год" => "every year",
        _ => return None,
    })
}

pub fn shown(russian: &str) -> String {
    match lang::current() {
        Locale::Ru => russian.to_string(),
        Locale::En => text(russian).unwrap_or(russian).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn названия_способов_оплаты_переводятся_только_для_английского() {
        assert_eq!(shown("Банковская карта"), "Банковская карта");
        lang::sync_scope(Locale::En, || {
            assert_eq!(shown("Банковская карта"), "Bank card");
            assert_eq!(shown("Мой способ"), "Мой способ");
        });
    }
}
