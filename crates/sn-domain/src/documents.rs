use ammonia::{Builder, UrlRelative};
use std::collections::{HashMap, HashSet};
use std::fmt;

pub const PATH_MAX: usize = 80;
pub const PATH_DEPTH: usize = 3;
pub const BODY_MAX: usize = 200_000;
pub const DESCRIPTION_MAX: usize = 160;

const TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "caption",
    "code",
    "dd",
    "del",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "ins",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

const DROPPED: &[&str] = &[
    "applet", "audio", "button", "canvas", "embed", "frame", "frameset", "head", "iframe", "math",
    "noembed", "noframes", "noscript", "object", "script", "select", "style", "svg", "template",
    "textarea", "title", "video", "xmp",
];

const SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

const RESERVED: &[(&str, &str)] = &[
    ("admin", "админка"),
    ("api", "API панели"),
    ("app", "мини-приложение"),
    ("assets", "файлы сайта"),
    ("cabinet", "личный кабинет"),
    ("favicon.ico", "значок сайта"),
    ("index.html", "главная страница"),
    ("m", "страницы модулей"),
    ("open-app", "переход в приложение для VPN"),
    ("pay", "страницы оплаты"),
    ("robots.txt", "правила для поисковиков"),
    ("service-worker.js", "служебный файл сайта"),
    ("sub", "подписки"),
    ("sw.js", "служебный файл сайта"),
    ("verify", "проверка «Я не робот»"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathProblem {
    Empty,
    Chars,
    Shape,
    TooLong,
    TooDeep,
    Reserved { path: String, owner: &'static str },
}

impl fmt::Display for PathProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathProblem::Empty => f.write_str(
                "Укажите адрес страницы, например privacy-policy: главная страница сайта занята лендингом",
            ),
            PathProblem::Chars => f.write_str(
                "В адресе страницы можно использовать только латинские буквы, цифры, дефис, подчёркивание и /",
            ),
            PathProblem::Shape => f.write_str(
                "Каждая часть адреса должна начинаться и заканчиваться буквой или цифрой, например legal/privacy-policy",
            ),
            PathProblem::TooLong => write!(f, "Адрес страницы длиннее {PATH_MAX} символов"),
            PathProblem::TooDeep => write!(
                f,
                "В адресе страницы может быть не больше {PATH_DEPTH} частей через /"
            ),
            PathProblem::Reserved { path, owner } => {
                write!(f, "Адрес {path} занят панелью: там {owner}. Выберите другой")
            }
        }
    }
}

fn reserved_owner(segment: &str) -> Option<&'static str> {
    RESERVED
        .iter()
        .find(|(name, _)| *name == segment)
        .map(|(_, owner)| *owner)
}

fn segment_ok(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    match (bytes.first(), bytes.last()) {
        (Some(first), Some(last)) => first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric(),
        _ => false,
    }
}

pub fn clean_path(raw: &str) -> Result<String, PathProblem> {
    let trimmed = raw.trim().trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(PathProblem::Empty);
    }
    let lower = trimmed.to_ascii_lowercase();
    let first = lower.split('/').next().unwrap_or("");
    if let Some(owner) = reserved_owner(first) {
        return Err(PathProblem::Reserved {
            path: format!("/{first}"),
            owner,
        });
    }
    if !lower
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '/'))
    {
        return Err(PathProblem::Chars);
    }
    let segments: Vec<&str> = lower.split('/').collect();
    if !segments.iter().all(|segment| segment_ok(segment)) {
        return Err(PathProblem::Shape);
    }
    if segments.len() > PATH_DEPTH {
        return Err(PathProblem::TooDeep);
    }
    let path = format!("/{lower}");
    if path.len() > PATH_MAX {
        return Err(PathProblem::TooLong);
    }
    Ok(path)
}

pub fn is_page_path(path: &str) -> bool {
    clean_path(path).is_ok_and(|clean| clean == path)
}

pub fn first_segment(path: &str) -> &str {
    path.trim_start_matches('/').split('/').next().unwrap_or("")
}

fn sanitizer() -> Builder<'static> {
    let mut builder = Builder::empty();
    let attributes: HashMap<&str, HashSet<&str>> = HashMap::from([
        ("a", HashSet::from(["href", "title"])),
        ("ol", HashSet::from(["start", "reversed"])),
        ("td", HashSet::from(["colspan", "rowspan"])),
        ("th", HashSet::from(["colspan", "rowspan", "scope"])),
    ]);
    builder
        .tags(TAGS.iter().copied().collect())
        .clean_content_tags(DROPPED.iter().copied().collect())
        .tag_attributes(attributes)
        .generic_attributes(HashSet::new())
        .url_schemes(SCHEMES.iter().copied().collect())
        .url_relative(UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer"))
        .strip_comments(true);
    builder
}

fn tag_end(tag: &str) -> Option<usize> {
    let mut quoted = false;
    for (at, c) in tag.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '>' if !quoted => return Some(at + 1),
            _ => {}
        }
    }
    None
}

fn unwrap_bare_links(html: String) -> String {
    if !html.contains("<a ") {
        return html;
    }
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(at) = rest.find("<a ") {
        out.push_str(&rest[..at]);
        let Some(end) = tag_end(&rest[at..]).map(|e| at + e) else {
            out.push_str(&rest[at..]);
            return out;
        };
        let tag = &rest[at..end];
        if tag.contains(" href=\"") {
            out.push_str(tag);
            rest = &rest[end..];
            continue;
        }
        rest = &rest[end..];
        match rest.find("</a>") {
            Some(close) => {
                out.push_str(&rest[..close]);
                rest = &rest[close + "</a>".len()..];
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

const PASTE_TAGS: &[&str] = &[
    "a",
    "b",
    "big",
    "blockquote",
    "br",
    "code",
    "dd",
    "del",
    "div",
    "dl",
    "dt",
    "em",
    "font",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "i",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "s",
    "samp",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tg-emoji",
    "tg-spoiler",
    "th",
    "thead",
    "tr",
    "tt",
    "u",
    "ul",
];

const PASTE_STYLED: &[&str] = &[
    "b", "big", "del", "div", "em", "font", "i", "li", "mark", "p", "s", "small", "span", "strike",
    "strong", "td", "u",
];

pub fn clean_paste(raw: &str) -> String {
    let mut attributes: HashMap<&str, HashSet<&str>> = PASTE_STYLED
        .iter()
        .map(|tag| (*tag, HashSet::from(["style"])))
        .collect();
    attributes.insert("a", HashSet::from(["href"]));
    attributes.insert("span", HashSet::from(["style", "class"]));
    attributes.insert("tg-emoji", HashSet::from(["emoji-id"]));
    attributes.insert("code", HashSet::from(["class"]));
    attributes.insert("blockquote", HashSet::from(["expandable"]));
    let mut builder = Builder::empty();
    builder
        .tags(PASTE_TAGS.iter().copied().collect())
        .clean_content_tags(DROPPED.iter().copied().collect())
        .tag_attributes(attributes)
        .generic_attributes(HashSet::new())
        .url_schemes(HashSet::from(["http", "https", "tg"]))
        .url_relative(UrlRelative::Deny)
        .link_rel(None)
        .strip_comments(true);
    builder.clean(raw).to_string()
}

pub fn clean_html(raw: &str) -> String {
    let cleaned = unwrap_bare_links(sanitizer().clean(raw).to_string());
    let trimmed = cleaned.trim();
    if trimmed.len() == cleaned.len() {
        cleaned
    } else {
        trimmed.to_string()
    }
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let code = name.strip_prefix('#')?;
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value)
        }
    }
}

const BLOCKS: &[&str] = &[
    "p",
    "br",
    "div",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "tr",
    "td",
    "th",
    "blockquote",
    "pre",
    "hr",
    "dt",
    "dd",
    "caption",
    "table",
];

pub fn plain_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(c) = rest.chars().next() {
        match c {
            '<' => {
                let end = rest.find('>').map_or(rest.len(), |at| at + 1);
                let tag = rest[1..end.saturating_sub(1).max(1)]
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if BLOCKS.contains(&tag.as_str()) {
                    out.push(' ');
                }
                rest = &rest[end..];
            }
            '&' => {
                let end = rest[1..].find(';').map(|at| at + 1);
                match end
                    .filter(|end| *end <= 12)
                    .and_then(|end| entity(&rest[1..end]).map(|ch| (end, ch)))
                {
                    Some((end, ch)) => {
                        out.push(ch);
                        rest = &rest[end + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            c => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn text_len(html: &str) -> usize {
    plain_text(html).chars().count()
}

fn sentence_end(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    chars.windows(3).rev().find_map(|w| {
        let [(_, before), (at, mark), (_, after)] = [w[0], w[1], w[2]];
        (matches!(mark, '.' | '!' | '?') && after == ' ' && !before.is_ascii_digit())
            .then_some(at + mark.len_utf8())
    })
}

pub fn description(html: &str) -> Option<String> {
    let text = plain_text(html);
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= DESCRIPTION_MAX {
        return Some(text);
    }
    let mut cut: String = text.chars().take(DESCRIPTION_MAX).collect();
    if let Some(end) = sentence_end(&cut).filter(|at| *at > cut.len() / 2) {
        cut.truncate(end);
        return Some(cut);
    }
    if let Some(space) = cut.rfind(' ').filter(|at| *at > cut.len() / 2) {
        cut.truncate(space);
    }
    Some(format!(
        "{}…",
        cut.trim_end_matches([',', '.', ';', ':', ' '])
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn адрес_приводится_к_виду_со_слешем_и_маленькими_буквами() {
        assert_eq!(clean_path("privacy-policy").unwrap(), "/privacy-policy");
        assert_eq!(clean_path(" /Privacy-Policy/ ").unwrap(), "/privacy-policy");
        assert_eq!(clean_path("legal/offer_2026").unwrap(), "/legal/offer_2026");
        assert_eq!(clean_path("a/b/c").unwrap(), "/a/b/c");
        assert!(is_page_path("/privacy-policy"));
        assert!(!is_page_path("/Privacy-Policy"));
        assert!(!is_page_path("privacy-policy"));
        assert_eq!(first_segment("/legal/offer"), "legal");
    }

    #[test]
    fn адреса_панели_заняты() {
        for raw in [
            "admin",
            "/admin/settings",
            "cabinet",
            "app",
            "/app/plans",
            "api",
            "api/public/site",
            "m",
            "open-app",
            "pay",
            "verify",
            "assets",
            "sw.js",
            "service-worker.js",
            "robots.txt",
            "index.html",
            "favicon.ico",
            "sub",
            "ADMIN",
        ] {
            match clean_path(raw) {
                Err(PathProblem::Reserved { path, owner }) => {
                    assert!(path.starts_with('/'), "{raw}");
                    assert!(!owner.is_empty(), "{raw}");
                }
                other => panic!("{raw}: {other:?}"),
            }
        }
        let text = clean_path("/admin").unwrap_err().to_string();
        assert!(text.contains("/admin"), "{text}");
        assert!(text.contains("админка"), "{text}");
        assert!(clean_path("administration").is_ok());
        assert!(clean_path("apps").is_ok());
        assert!(clean_path("payments-policy").is_ok());
    }

    #[test]
    fn плохие_адреса_получают_понятную_ошибку() {
        assert_eq!(clean_path(""), Err(PathProblem::Empty));
        assert_eq!(clean_path("/"), Err(PathProblem::Empty));
        assert_eq!(clean_path("политика"), Err(PathProblem::Chars));
        assert_eq!(clean_path("privacy policy"), Err(PathProblem::Chars));
        assert_eq!(clean_path("privacy.html"), Err(PathProblem::Chars));
        assert_eq!(clean_path("a?b=1"), Err(PathProblem::Chars));
        assert_eq!(clean_path("..%2fadmin"), Err(PathProblem::Chars));
        assert_eq!(clean_path("-privacy"), Err(PathProblem::Shape));
        assert_eq!(clean_path("legal//offer"), Err(PathProblem::Shape));
        assert_eq!(clean_path("legal/-/offer"), Err(PathProblem::Shape));
        assert_eq!(clean_path("a/b/c/d"), Err(PathProblem::TooDeep));
        assert_eq!(clean_path(&"a".repeat(PATH_MAX)), Err(PathProblem::TooLong));
        for problem in [
            PathProblem::Empty,
            PathProblem::Chars,
            PathProblem::Shape,
            PathProblem::TooLong,
            PathProblem::TooDeep,
        ] {
            assert!(!problem.to_string().is_empty());
        }
    }

    #[test]
    fn скрипты_удаляются_вместе_с_содержимым() {
        let clean = clean_html(
            "<p>До</p><script>alert(1)</script><SCRIPT src=\"https://evil.example/x.js\"></SCRIPT><p>После</p>",
        );
        assert_eq!(clean, "<p>До</p><p>После</p>");
        let clean = clean_html("<p>a<script>document.cookie</script>b</p>");
        assert_eq!(clean, "<p>ab</p>");
        assert!(!clean_html("<svg><script>alert(1)</script></svg>").contains("script"));
        assert!(
            !clean_html("<math><mtext><script>alert(1)</script></mtext></math>").contains("alert")
        );
        assert!(!clean_html("<template><script>alert(1)</script></template>").contains("alert"));
    }

    #[test]
    fn обработчики_событий_снимаются() {
        let clean = clean_html(
            "<p onclick=\"alert(1)\" onmouseover='x()'>Текст</p><a href=\"https://example.com\" onfocus=\"alert(1)\" autofocus>ссылка</a>",
        );
        assert_eq!(
            clean,
            "<p>Текст</p><a href=\"https://example.com\" rel=\"noopener noreferrer\">ссылка</a>"
        );
        let clean = clean_html("<img src=x onerror=alert(1)><b ONLOAD=\"x()\">жирный</b>");
        assert_eq!(clean, "<b>жирный</b>");
        let clean = clean_html("<details open ontoggle=alert(1)><summary>x</summary></details>");
        assert!(!clean.contains("ontoggle"), "{clean}");
        assert!(!clean.contains("<details"), "{clean}");
        for raw in [
            "<body onload=alert(1)>текст",
            "<div onpointerenter=alert(1)>текст</div>",
            "<p/onclick=alert(1)>текст</p>",
            "<table background=\"javascript:alert(1)\"><tr><td>1</td></tr></table>",
        ] {
            let clean = clean_html(raw);
            assert!(!clean.contains("alert"), "{raw} -> {clean}");
            assert!(
                clean.contains("текст") || clean.contains("<td>1</td>"),
                "{raw} -> {clean}"
            );
        }
    }

    #[test]
    fn ссылки_javascript_и_data_теряют_адрес() {
        for href in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:alert(1)",
            "java&#x09;script:alert(1)",
            "java\nscript:alert(1)",
            "&#106;&#97;&#118;&#97;&#115;&#99;&#114;&#105;&#112;&#116;&#58;alert(1)",
            "vbscript:msgbox(1)",
            "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
            "file:///etc/passwd",
        ] {
            let clean = clean_html(&format!("<p><a href=\"{href}\">клик</a> и дальше</p>"));
            assert_eq!(clean, "<p>клик и дальше</p>", "{href}");
        }
        assert_eq!(
            clean_html("<a href=\"mailto:help@example.com\">почта</a>"),
            "<a href=\"mailto:help@example.com\" rel=\"noopener noreferrer\">почта</a>"
        );
        assert_eq!(
            clean_html("<a href=\"/cabinet\" target=\"_blank\">кабинет</a>"),
            "<a href=\"/cabinet\" rel=\"noopener noreferrer\">кабинет</a>"
        );
        assert_eq!(
            clean_html("<a href=\"#p2\">пункт 2</a>"),
            "<a href=\"#p2\" rel=\"noopener noreferrer\">пункт 2</a>"
        );
        assert_eq!(
            clean_html(
                "<p><a name=\"p2\">2.</a> <a title=\"x\">Термины</a> <a href=\"/offer\" title=\"Оферта\">оферта</a></p>"
            ),
            "<p>2. Термины <a href=\"/offer\" title=\"Оферта\" rel=\"noopener noreferrer\">оферта</a></p>"
        );
        assert_eq!(
            clean_html(
                "<a title=\"a > b\">текст</a> <a title=\"x>y\" href=\"https://example.com\">ссылка</a>"
            ),
            "текст <a title=\"x&gt;y\" href=\"https://example.com\" rel=\"noopener noreferrer\">ссылка</a>"
        );
    }

    #[test]
    fn фреймы_объекты_и_формы_удаляются() {
        for raw in [
            "<iframe src=\"https://evil.example\"></iframe>",
            "<iframe srcdoc=\"&lt;script&gt;alert(1)&lt;/script&gt;\"></iframe>",
            "<frameset><frame src=\"https://evil.example\"></frameset>",
            "<object data=\"https://evil.example/x.swf\"></object>",
            "<embed src=\"https://evil.example/x.swf\">",
            "<applet code=\"x\"></applet>",
            "<video src=x onerror=alert(1)></video>",
            "<audio src=x></audio>",
            "<base href=\"https://evil.example/\">",
            "<meta http-equiv=\"refresh\" content=\"0;url=https://evil.example\">",
            "<link rel=\"stylesheet\" href=\"https://evil.example/x.css\">",
            "<form action=\"https://evil.example\"><input name=\"card\"><button formaction=\"https://evil.example\">Оплатить</button></form>",
        ] {
            let clean = clean_html(raw);
            assert!(!clean.contains("evil"), "{raw} -> {clean}");
            for tag in [
                "<iframe", "<frame", "<object", "<embed", "<applet", "<video", "<audio", "<base",
                "<meta", "<link", "<form", "<input", "<button",
            ] {
                assert!(!clean.contains(tag), "{raw} -> {clean}");
            }
        }
    }

    #[test]
    fn стили_и_url_внутри_стилей_удаляются() {
        let clean = clean_html(
            "<p style=\"background:url(https://evil.example/track.png)\">Текст</p><style>body{background:url(javascript:alert(1))}</style><span style=\"color:red\">красный</span>",
        );
        assert_eq!(clean, "<p>Текст</p><span>красный</span>");
        assert!(!clean_html("<div style=\"background-image: url('x')\">a</div>").contains("url"));
        assert!(
            !clean_html("<p class=\"MsoNormal\" id=\"root\" lang=\"ru\">Word</p>").contains('=')
        );
    }

    #[test]
    fn вложенные_обходы_разбора_не_оживают() {
        for raw in [
            "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\"></noscript>",
            "<scr<script>ipt>alert(1)</script>",
            "<<script>script>alert(1)<</script>/script>",
            "<!--<img src=x onerror=alert(1)>-->",
            "<p><![CDATA[<img src=x onerror=alert(1)>]]></p>",
            "<textarea><img src=x onerror=alert(1)></textarea>",
            "<title><img src=x onerror=alert(1)></title>",
            "<xmp><img src=x onerror=alert(1)></xmp>",
            "<select><option><img src=x onerror=alert(1)></option></select>",
        ] {
            let clean = clean_html(raw);
            assert!(!clean.contains("onerror"), "{raw} -> {clean}");
            assert!(!clean.contains("<img"), "{raw} -> {clean}");
            assert!(!clean.contains("<script"), "{raw} -> {clean}");
            assert_eq!(clean_html(&clean), clean, "{raw}");
        }
    }

    #[test]
    fn оформление_документа_сохраняется() {
        let raw = "<h1>Оферта</h1><h2>1. Термины</h2><h3>1.1</h3><p><strong>Жирный</strong>, <b>b</b>, <em>курсив</em>, <i>i</i>, <u>u</u>, <s>s</s>, H<sub>2</sub>O, x<sup>2</sup>.</p><ul><li>раз</li></ul><ol start=\"3\"><li>три</li></ol><blockquote>цитата</blockquote><hr><table><thead><tr><th colspan=\"2\" scope=\"col\">Тариф</th></tr></thead><tbody><tr><td rowspan=\"1\">1</td><td>2</td></tr></tbody></table><p>строка<br>вторая</p>";
        let clean = clean_html(raw);
        for part in [
            "<h1>Оферта</h1>",
            "<h2>1. Термины</h2>",
            "<strong>Жирный</strong>",
            "<em>курсив</em>",
            "<u>u</u>",
            "<sub>2</sub>",
            "<ul><li>раз</li></ul>",
            "<ol start=\"3\"><li>три</li></ol>",
            "<blockquote>цитата</blockquote>",
            "<hr>",
            "<th colspan=\"2\" scope=\"col\">Тариф</th>",
            "<td rowspan=\"1\">1</td>",
            "строка<br>вторая",
        ] {
            assert!(clean.contains(part), "{part} нет в {clean}");
        }
        assert_eq!(clean_html(&clean), clean);
    }

    #[test]
    fn текст_и_описание_берутся_из_разметки() {
        let html = clean_html(
            "<h2>1. Термины</h2><p>Сервис&nbsp;&laquo;VPN&raquo; &amp; приложения &lt;b&gt;</p><ul><li>раз</li><li>два</li></ul>",
        );
        assert_eq!(
            plain_text(&html),
            "1. Термины Сервис «VPN» & приложения <b> раз два"
        );
        assert_eq!(text_len("<p>  </p><br>"), 0);
        assert_eq!(text_len("<p>Да</p>"), 2);
        assert_eq!(description("<p></p>"), None);
        let long = format!("<p>{}</p>", "слово ".repeat(100));
        let cut = description(&long).unwrap();
        assert!(cut.chars().count() <= DESCRIPTION_MAX + 1, "{cut}");
        assert!(cut.ends_with("слово…"), "{cut}");
        let sentences = format!(
            "<p>{} Конец первой части.</p><h2>1. Термины</h2><p>{}</p>",
            "Текст ".repeat(18),
            "дальше ".repeat(30)
        );
        let cut = description(&sentences).unwrap();
        assert!(cut.ends_with("Конец первой части."), "{cut}");
    }

    #[test]
    fn вставка_в_описание_тарифа_очищается_без_обработчиков() {
        let clean = clean_paste(
            "<meta charset=\"utf-8\"><b style=\"font-weight:normal\" id=\"docs-guid\"><p dir=\"ltr\"><span style=\"font-weight:700\" onclick=\"x()\">Жирный</span></p></b><img src=x onerror=\"alert(1)\"><script>alert(2)</script><a href=\"javascript:alert(3)\">ссылка</a><a href=\"https://t.me/x\" target=\"_blank\">t.me</a><span class=\"tg-spoiler\">тайна</span><tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji>",
        );
        for bad in [
            "onclick",
            "onerror",
            "<img",
            "<script",
            "alert",
            "javascript:",
            "target",
            " id=",
            "dir=",
        ] {
            assert!(!clean.contains(bad), "{bad} остался в {clean}");
        }
        for part in [
            "<b style=\"font-weight:normal\">",
            "<span style=\"font-weight:700\">Жирный</span>",
            "<a>ссылка</a>",
            "<a href=\"https://t.me/x\">t.me</a>",
            "<span class=\"tg-spoiler\">тайна</span>",
            "<tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji>",
        ] {
            assert!(clean.contains(part), "{part} нет в {clean}");
        }
    }
}
