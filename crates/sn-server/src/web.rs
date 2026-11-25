use crate::error::ApiError;
use crate::security::Section;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use sn_domain::client::Locale;
use sn_domain::format;
use sn_domain::module::ModuleId;
use sn_domain::settings::CoreSettings;
use sn_services::branding::{TabIcon, tab_icon, touch_icon};
use sn_services::documents::{PublicPage, find_page};
use std::path::Path;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const RETIRED_WORKER: &str = include_str!("retired_worker.js");
const PREVIEW_BOTS: [&str; 9] = [
    "TelegramBot",
    "Twitterbot",
    "facebookexternalhit",
    "Slackbot",
    "Discordbot",
    "WhatsApp",
    "vkShare",
    "LinkedInBot",
    "SkypeUriPreview",
];

pub fn preview_bot(user_agent: &str) -> Option<&'static str> {
    let lower = user_agent.to_ascii_lowercase();
    PREVIEW_BOTS
        .into_iter()
        .find(|bot| lower.contains(&bot.to_ascii_lowercase()))
}

pub fn robots_txt(indexing: bool) -> String {
    let mut out = String::new();
    for bot in PREVIEW_BOTS {
        out.push_str(&format!("User-agent: {bot}\n"));
    }
    out.push_str("Allow: /\n\n");
    out.push_str("User-agent: *\n");
    if indexing {
        out.push_str(
            "Disallow: /admin\nDisallow: /cabinet\nDisallow: /app\nDisallow: /api/\nAllow: /\n",
        );
    } else {
        out.push_str("Disallow: /\n");
    }
    out
}
const REVALIDATE: &str = "no-cache";
const ICON: &str = "/api/public/icon.svg";
const MINI_APP: &str = "/app";
const CABINET: &str = "/cabinet";
const ROOT: &str = "<div id=\"root\">";

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn embeddable(path: &str) -> bool {
    path == "/app" || path.starts_with("/app/") || path.starts_with("/verify/")
}

fn landing_preview(path: &str) -> bool {
    [
        "/admin/landing-preview",
        "/admin/document-preview",
        "/admin/mini-preview",
        "/admin/cabinet-preview",
    ]
    .iter()
    .any(|frame| path == *frame || path.starts_with(&format!("{frame}/")))
}

fn frame_option(path: &str) -> Option<&'static str> {
    if embeddable(path) {
        None
    } else if landing_preview(path) {
        Some("SAMEORIGIN")
    } else {
        Some("DENY")
    }
}

fn section(path: &str) -> Section {
    if path.starts_with("/verify/") {
        return Section::MiniApp;
    }
    match Page::of(path) {
        Page::MiniApp => Section::MiniApp,
        Page::Admin if landing_preview(path) => Section::AdminFrame,
        Page::Admin => Section::Admin,
        Page::Landing | Page::Cabinet | Page::Other => Section::Site,
    }
}

fn retired_worker(dir: &Path, path: &str) -> bool {
    matches!(path, "/sw.js" | "/service-worker.js")
        && !dir.join(path.trim_start_matches('/')).is_file()
}

fn retired_worker_script() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        RETIRED_WORKER,
    )
        .into_response()
}

fn hashed_asset(path: &str) -> bool {
    path.starts_with("/assets/")
}

fn renders_page(path: &str) -> bool {
    path == "/index.html"
        || !path
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains('.'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Landing,
    Cabinet,
    MiniApp,
    Admin,
    Other,
}

impl Page {
    fn of(path: &str) -> Page {
        match path.trim_start_matches('/').split('/').next().unwrap_or("") {
            "" | "index.html" => Page::Landing,
            "cabinet" => Page::Cabinet,
            "app" => Page::MiniApp,
            "admin" => Page::Admin,
            _ => Page::Other,
        }
    }
}

fn opens_app(settings: &CoreSettings, page: Page) -> bool {
    settings.client_access.miniapp && matches!(page, Page::Landing | Page::Cabinet)
}

fn in_frame(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|dest| dest.eq_ignore_ascii_case("iframe"))
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn brand_tag(brand: &str) -> String {
    format!(
        "<meta name=\"application-name\" content=\"{}\">",
        escape_html(brand)
    )
}

fn icon_tags(settings: &CoreSettings) -> Vec<String> {
    let (icon, icon_type) = match tab_icon(settings) {
        TabIcon::Image(image, meta) => (meta.path(image), meta.kind.content_type()),
        TabIcon::Mark => (ICON.to_string(), "image/svg+xml"),
    };
    let mut tags = vec![format!(
        "<link rel=\"icon\" type=\"{icon_type}\" href=\"{}\">",
        escape_html(&icon)
    )];
    if let Some(touch) = touch_icon(settings) {
        tags.push(format!(
            "<link rel=\"apple-touch-icon\" href=\"{}\">",
            escape_html(&touch)
        ));
    }
    tags
}

fn document_head(settings: &CoreSettings, page: &PublicPage) -> String {
    let brand = settings.brand_name.as_str();
    let robots = if settings.site.indexing {
        "index, follow"
    } else {
        "noindex"
    };
    let mut tags = vec![
        format!(
            "<title>{}</title>",
            escape_html(&format!("{} — {brand}", page.title))
        ),
        brand_tag(brand),
        format!("<meta name=\"robots\" content=\"{robots}\">"),
    ];
    tags.extend(icon_tags(settings));
    if let Some(description) = &page.description {
        tags.push(format!(
            "<meta name=\"description\" content=\"{}\">",
            escape_html(description)
        ));
    }
    tags.push("<meta property=\"og:type\" content=\"article\">".to_string());
    tags.push(format!(
        "<meta property=\"og:site_name\" content=\"{}\">",
        escape_html(brand)
    ));
    tags.push(format!(
        "<meta property=\"og:title\" content=\"{}\">",
        escape_html(&page.title)
    ));
    if let Some(description) = &page.description {
        tags.push(format!(
            "<meta property=\"og:description\" content=\"{}\">",
            escape_html(description)
        ));
    }
    if let Some(base) = settings
        .public_url
        .as_deref()
        .map(|url| url.trim_end_matches('/'))
    {
        let url = escape_html(&format!("{base}{}", page.path));
        tags.push(format!("<meta property=\"og:url\" content=\"{url}\">"));
        if settings.site.indexing {
            tags.push(format!("<link rel=\"canonical\" href=\"{url}\">"));
        }
    }
    tags.push("<meta name=\"twitter:card\" content=\"summary\">".to_string());
    tags.push(format!(
        "<meta name=\"twitter:title\" content=\"{}\">",
        escape_html(&page.title)
    ));
    tags.join("\n    ")
}

fn document_body(page: &PublicPage) -> String {
    let updated = match page.lang {
        Locale::Ru => "Обновлено",
        Locale::En => "Updated",
    };
    format!(
        "<noscript><article class=\"dp-noscript\"><h1>{}</h1><p>{updated} {}</p>{}</article></noscript>",
        escape_html(&page.title),
        format::date_in(page.updated_at.date_naive(), page.lang),
        page.html
    )
}

fn script_needed(locale: Locale) -> &'static str {
    match locale {
        Locale::Ru => {
            "<noscript><p>Для работы сайта включите JavaScript в настройках браузера.</p></noscript>"
        }
        Locale::En => {
            "<noscript><p>Please turn on JavaScript in your browser to use this site.</p></noscript>"
        }
    }
}

fn with_body(mut html: String, body: &str) -> String {
    match html.find(ROOT).or_else(|| html.find("</body>")) {
        Some(at) => html.insert_str(at, body),
        None => html.push_str(body),
    }
    html
}

fn head(settings: &CoreSettings, page: Page, locale: Locale) -> String {
    let brand = settings.brand_name.as_str();
    let site_title = settings.site.title.as_deref().unwrap_or(brand);
    let title = match page {
        Page::Landing | Page::Other => site_title.to_string(),
        Page::Cabinet => match locale {
            Locale::Ru => format!("Личный кабинет — {brand}"),
            Locale::En => format!("Your account — {brand}"),
        },
        Page::MiniApp => brand.to_string(),
        Page::Admin => format!("Админка — {brand}"),
    };
    let robots = match page {
        Page::Landing if settings.site.indexing => "index, follow",
        Page::Admin => "noindex, nofollow",
        _ => "noindex",
    };
    let base = settings
        .public_url
        .as_deref()
        .map(|url| url.trim_end_matches('/'));
    let logo = settings.logo_url();
    let description = settings.site.description.as_deref();
    let mut tags = vec![
        format!("<title>{}</title>", escape_html(&title)),
        brand_tag(brand),
        format!("<meta name=\"robots\" content=\"{robots}\">"),
    ];
    tags.extend(icon_tags(settings));
    if page == Page::Admin {
        return tags.join("\n    ");
    }
    if opens_app(settings, page) {
        tags.push(format!(
            "<meta name=\"telegram-app\" content=\"{MINI_APP}\">"
        ));
    }
    if let Some(description) = description {
        tags.push(format!(
            "<meta name=\"description\" content=\"{}\">",
            escape_html(description)
        ));
    }
    tags.push("<meta property=\"og:type\" content=\"website\">".to_string());
    tags.push(format!(
        "<meta property=\"og:site_name\" content=\"{}\">",
        escape_html(brand)
    ));
    tags.push(format!(
        "<meta property=\"og:title\" content=\"{}\">",
        escape_html(site_title)
    ));
    if let Some(description) = description {
        tags.push(format!(
            "<meta property=\"og:description\" content=\"{}\">",
            escape_html(description)
        ));
    }
    if let Some(base) = base {
        let canonical = match page {
            Page::Landing => Some("/"),
            Page::Cabinet => Some("/cabinet"),
            _ => None,
        };
        if let Some(path) = canonical {
            let url = escape_html(&format!("{base}{path}"));
            tags.push(format!("<meta property=\"og:url\" content=\"{url}\">"));
            if page == Page::Landing && settings.site.indexing {
                tags.push(format!("<link rel=\"canonical\" href=\"{url}\">"));
            }
        }
        if let Some(logo) = &logo {
            tags.push(format!(
                "<meta property=\"og:image\" content=\"{}\">",
                escape_html(&format!("{base}{logo}"))
            ));
        }
    }
    tags.push("<meta name=\"twitter:card\" content=\"summary\">".to_string());
    tags.push(format!(
        "<meta name=\"twitter:title\" content=\"{}\">",
        escape_html(site_title)
    ));
    if let Some(description) = description {
        tags.push(format!(
            "<meta name=\"twitter:description\" content=\"{}\">",
            escape_html(description)
        ));
    }
    tags.join("\n    ")
}

fn cut(html: &mut String, open: &str, close: &str) -> Option<usize> {
    let start = html.find(open)?;
    let end = html[start..].find(close)? + start + close.len();
    let line = html[..start].rfind('\n').map_or(0, |at| at + 1);
    let rest = &html[end..];
    let blank_line = html[line..start].trim().is_empty() && rest.starts_with('\n');
    if blank_line && open.starts_with("<meta") {
        html.replace_range(line..end + 1, "");
        return Some(line);
    }
    html.replace_range(start..end, "");
    Some(start)
}
fn with_lang(html: String, locale: Locale) -> String {
    match locale {
        Locale::Ru => html,
        Locale::En => html.replacen("<html lang=\"ru\">", "<html lang=\"en\">", 1),
    }
}

fn inject(template: &str, head: &str) -> String {
    let mut html = template.to_string();
    while cut(&mut html, "<meta name=\"robots\"", ">").is_some() {}
    let at = cut(&mut html, "<title", "</title>").or_else(|| html.find("</head>"));
    match at {
        Some(at) => html.insert_str(at, head),
        None => html.insert_str(0, head),
    }
    html
}

pub async fn fallback(State(state): State<AppState>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();
    if is_api(&path) {
        return ApiError::not_found().into_response();
    }
    let Some(dir) = state.config.web_dir.clone() else {
        return ApiError::not_found().into_response();
    };
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if retired_worker(&dir, &path) {
        return retired_worker_script();
    }
    if renders_page(&path) {
        let wanted = crate::lang::from_cookie(request.headers());
        if let Some(response) = document(&state, &dir, &path, request.uri().query(), wanted).await {
            return response;
        }
        if in_frame(request.headers()) && opens_app(&state.services.settings(), Page::of(&path)) {
            return instead(MINI_APP, request.uri().query());
        }
        if Page::of(&path) == Page::Landing
            && !state.services.modules().is_enabled(ModuleId::Landing)
        {
            return instead(CABINET, request.uri().query());
        }
        if let Some(bot) = request
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .and_then(preview_bot)
        {
            tracing::info!(bot, path = %path, "сборщик превью ссылок открыл страницу");
        }
        return page(&state, &dir, &path, wanted).await;
    }
    serve(&state, &dir, &path, request).await
}

fn instead(path: &str, query: Option<&str>) -> Response {
    let target = match query.filter(|q| !q.is_empty()) {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let mut response = Redirect::temporary(&target).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn document(
    state: &AppState,
    dir: &Path,
    path: &str,
    query: Option<&str>,
    wanted: Option<Locale>,
) -> Option<Response> {
    let runtime = state.services.bot_content();
    let trimmed = path.trim_end_matches('/');
    find_page(&runtime.documents, trimmed)?;
    if trimmed != path {
        let target = match query.filter(|q| !q.is_empty()) {
            Some(query) => format!("{trimmed}?{query}"),
            None => trimmed.to_string(),
        };
        return Some(Redirect::permanent(&target).into_response());
    }
    let settings = state.services.settings();
    let locale = settings.languages.pick(wanted);
    let page = match state.services.documents().page(path, locale).await {
        Ok(Some(page)) => page,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(error = %err, path, "страница документа не загрузилась");
            return None;
        }
    };
    let template = tokio::fs::read_to_string(dir.join("index.html"))
        .await
        .ok()?;
    let html = with_lang(
        with_body(
            inject(&template, &document_head(&settings, &page)),
            &document_body(&page),
        ),
        page.lang,
    );
    let mut response = Html(html).into_response();
    protect(&mut response, state, path, REVALIDATE);
    Some(response)
}

async fn page(state: &AppState, dir: &Path, path: &str, wanted: Option<Locale>) -> Response {
    let Ok(template) = tokio::fs::read_to_string(dir.join("index.html")).await else {
        return ApiError::not_found().into_response();
    };
    let settings = state.services.settings();
    let page = Page::of(path);
    let locale = match page {
        Page::Admin => Locale::Ru,
        _ => settings.languages.pick(wanted),
    };
    let mut tags = head(&settings, page, locale);
    let style = match page {
        Page::Landing => state.services.landing().style(),
        Page::MiniApp => state.services.miniapp().style(),
        Page::Cabinet => state.services.cabinet().style(),
        Page::Admin | Page::Other => None,
    };
    if let Some(style) = style {
        tags.push_str(&format!(
            "\n    <link rel=\"preload\" as=\"style\" href=\"{}\">",
            escape_html(&style.path())
        ));
    }
    let html = with_lang(
        with_body(inject(&template, &tags), script_needed(locale)),
        locale,
    );
    let mut response = Html(html).into_response();
    protect(&mut response, state, path, REVALIDATE);
    response
}

fn protect(response: &mut Response, state: &AppState, path: &str, cache: &'static str) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Some(option) = frame_option(path) {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static(option));
    }
    crate::security::apply_with(
        headers,
        section(path),
        state.config.csp,
        state.services.env().ext_modules_loopback,
        captcha_origin(state),
    );
}

fn captcha_origin(state: &AppState) -> Option<crate::security::CaptchaSource> {
    use sn_modules::catalog::antibot::captcha::{Captcha, Provider};
    let modules = state.services.modules();
    if !modules.is_enabled(ModuleId::Antibot) {
        return None;
    }
    Captcha::of(&modules.config(ModuleId::Antibot)).map(|captcha| crate::security::CaptchaSource {
        origin: captcha.provider.origin(),
        inline_styles: captcha.provider == Provider::Yandex,
    })
}

async fn serve(state: &AppState, dir: &Path, path: &str, request: Request<Body>) -> Response {
    let index = dir.join("index.html");
    let service = ServeDir::new(dir)
        .precompressed_br()
        .precompressed_gzip()
        .fallback(ServeFile::new(index));
    let mut response = match service.oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    let cache = if hashed_asset(path) && response.status() == StatusCode::OK {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    protect(&mut response, state, path, cache);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_domain::image::ImageKind;
    use sn_domain::settings::{ImageMeta, SiteSettings};

    #[test]
    fn сборщики_превью_узнаются_и_пускаются_в_robots() {
        assert_eq!(
            preview_bot("TelegramBot (like TwitterBot)"),
            Some("TelegramBot")
        );
        assert_eq!(
            preview_bot(
                "facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)"
            ),
            Some("facebookexternalhit")
        );
        assert_eq!(
            preview_bot("Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X)"),
            None
        );
        let open = robots_txt(true);
        assert!(open.starts_with("User-agent: TelegramBot\n"));
        assert!(open.contains("User-agent: *\nDisallow: /admin\n"));
        assert!(open.ends_with("Allow: /\n"));
        let closed = robots_txt(false);
        assert!(closed.contains("User-agent: TelegramBot\n"));
        assert!(closed.ends_with("User-agent: *\nDisallow: /\n"));
    }

    #[test]
    fn адреса_api_не_уходят_в_интерфейс() {
        assert!(is_api("/api"));
        assert!(is_api("/api/admin/me"));
        assert!(!is_api("/apiary"));
        assert!(!is_api("/admin"));
    }

    #[test]
    fn во_фрейме_открываются_мини_приложение_и_превью_лендинга_из_админки() {
        assert!(embeddable("/app"));
        assert!(embeddable("/verify/0123456789abcdef"));
        assert_eq!(frame_option("/verify/0123456789abcdef"), None);
        assert!(!embeddable("/verifying"));
        assert!(embeddable("/app/plans"));
        assert!(!embeddable("/admin"));
        assert!(!embeddable("/application"));
        assert_eq!(frame_option("/app"), None);
        assert_eq!(frame_option("/app/plans"), None);
        assert_eq!(frame_option("/admin/landing-preview"), Some("SAMEORIGIN"));
        assert_eq!(
            frame_option("/admin/landing-preview/phone"),
            Some("SAMEORIGIN")
        );
        assert_eq!(frame_option("/admin/landing-previewer"), Some("DENY"));
        assert_eq!(frame_option("/admin/mini-preview"), Some("SAMEORIGIN"));
        assert_eq!(frame_option("/admin/mini-previewer"), Some("DENY"));
        assert_eq!(frame_option("/admin/settings/miniapp"), Some("DENY"));
        assert_eq!(frame_option("/admin"), Some("DENY"));
        assert_eq!(frame_option("/admin/settings/landing"), Some("DENY"));
        assert_eq!(frame_option("/"), Some("DENY"));
        assert_eq!(frame_option("/landing-preview"), Some("DENY"));
        assert_eq!(frame_option("/admin/document-preview"), Some("SAMEORIGIN"));
        assert_eq!(frame_option("/admin/document-previews"), Some("DENY"));
        assert_eq!(frame_option("/privacy-policy"), Some("DENY"));
    }

    #[test]
    fn у_каждого_раздела_своя_политика_csp() {
        assert_eq!(section("/"), Section::Site);
        assert_eq!(section("/cabinet/pay"), Section::Site);
        assert_eq!(section("/pay/42"), Section::Site);
        assert_eq!(section("/assets/index-1a2b.js"), Section::Site);
        assert_eq!(section("/app"), Section::MiniApp);
        assert_eq!(section("/app/plans"), Section::MiniApp);
        assert_eq!(section("/application"), Section::Site);
        assert_eq!(section("/admin"), Section::Admin);
        assert_eq!(section("/admin/m/ext/7"), Section::Admin);
        assert_eq!(section("/admin/landing-preview"), Section::AdminFrame);
        assert_eq!(section("/admin/mini-preview"), Section::AdminFrame);
        assert_eq!(section("/admin/landing-previewer"), Section::Admin);
        assert_eq!(section("/verify/0123456789abcdef"), Section::MiniApp);
        assert_eq!(section("/verifying"), Section::Site);
        assert_eq!(section("/admin/document-preview"), Section::AdminFrame);
        assert_eq!(section("/privacy-policy"), Section::Site);
        assert_eq!(section("/legal/offer"), Section::Site);
    }

    #[test]
    fn страницы_отличаются_от_файлов_по_точке_в_имени() {
        assert!(renders_page("/"));
        assert!(renders_page("/index.html"));
        assert!(renders_page("/cabinet/pay"));
        assert!(renders_page("/admin/clients/12"));
        assert!(!renders_page("/assets/index-1a2b.js"));
        assert!(!renders_page("/favicon.ico"));
        assert_eq!(Page::of("/"), Page::Landing);
        assert_eq!(Page::of("/cabinet"), Page::Cabinet);
        assert_eq!(Page::of("/app/plans"), Page::MiniApp);
        assert_eq!(Page::of("/admin/settings"), Page::Admin);
        assert_eq!(Page::of("/whatever"), Page::Other);
    }

    fn settings() -> CoreSettings {
        CoreSettings {
            brand_name: "Тихий VPN".into(),
            public_url: Some("https://vpn.example/".into()),
            site: SiteSettings {
                title: Some("Тихий VPN — \"быстрый\" <доступ>".into()),
                description: Some("Подписка & приложения".into()),
                indexing: true,
            },
            logo: Some(ImageMeta {
                tag: "0123456789abcdef".into(),
                kind: ImageKind::Png,
                width: 256,
                height: 256,
                size: 4096,
            }),
            ..CoreSettings::default()
        }
    }

    #[test]
    fn главная_получает_заголовок_описание_и_превью_с_экранированием() {
        let html = inject(
            "<!doctype html><html><head><meta charset=\"utf-8\" />\n<meta name=\"robots\" content=\"noindex\" />\n<title>STEALTHNET</title>\n</head><body></body></html>",
            &head(&settings(), Page::Landing, Locale::Ru),
        );
        assert!(!html.contains("STEALTHNET"));
        assert!(!html.contains("content=\"noindex\""));
        assert!(html.contains("<title>Тихий VPN — &quot;быстрый&quot; &lt;доступ&gt;</title>"));
        assert!(html.contains("<meta name=\"robots\" content=\"index, follow\">"));
        assert!(html.contains("<meta name=\"description\" content=\"Подписка &amp; приложения\">"));
        assert!(html.contains("<meta property=\"og:url\" content=\"https://vpn.example/\">"));
        assert!(html.contains("<link rel=\"canonical\" href=\"https://vpn.example/\">"));
        assert!(html.contains(
            "<meta property=\"og:image\" content=\"https://vpn.example/api/public/logo/0123456789abcdef.png\">"
        ));
        assert!(html.contains(
            "<link rel=\"icon\" type=\"image/png\" href=\"/api/public/logo/0123456789abcdef.png\">"
        ));
        assert!(html.contains(
            "<link rel=\"apple-touch-icon\" href=\"/api/public/logo/0123456789abcdef.png\">"
        ));
        assert!(html.find("<title>").unwrap() < html.find("</head>").unwrap());
        assert_eq!(html.matches("<title>").count(), 1);
    }

    #[test]
    fn админка_и_кабинет_закрыты_от_поиска_а_админка_без_описания() {
        let admin = head(&settings(), Page::Admin, Locale::Ru);
        assert!(admin.contains("<title>Админка — Тихий VPN</title>"));
        assert!(admin.contains("noindex, nofollow"));
        assert!(!admin.contains("description"));
        assert!(!admin.contains("og:"));
        let cabinet = head(&settings(), Page::Cabinet, Locale::Ru);
        assert!(cabinet.contains("<title>Личный кабинет — Тихий VPN</title>"));
        assert!(cabinet.contains("<meta name=\"robots\" content=\"noindex\">"));
        assert!(cabinet.contains("og:url\" content=\"https://vpn.example/cabinet\""));
        assert!(!cabinet.contains("canonical"));
        let mut hidden = settings();
        hidden.site.indexing = false;
        hidden.logo = None;
        hidden.public_url = None;
        let landing = head(&hidden, Page::Landing, Locale::Ru);
        assert!(landing.contains("<meta name=\"robots\" content=\"noindex\">"));
        assert!(
            landing.contains(
                "<link rel=\"icon\" type=\"image/svg+xml\" href=\"/api/public/icon.svg\">"
            )
        );
        assert!(!landing.contains("apple-touch-icon"));
        assert!(!landing.contains("og:image"));
        assert!(!landing.contains("og:url"));
    }

    #[test]
    fn главная_и_кабинет_знают_куда_вести_из_telegram_пока_приложение_включено() {
        let tag = "<meta name=\"telegram-app\" content=\"/app\">";
        let mut site = settings();
        for page in [Page::Landing, Page::Cabinet] {
            assert!(head(&site, page, Locale::Ru).contains(tag), "{page:?}");
        }
        for page in [Page::MiniApp, Page::Admin, Page::Other] {
            assert!(
                !head(&site, page, Locale::Ru).contains("telegram-app"),
                "{page:?}"
            );
        }
        site.client_access.miniapp = false;
        for page in [Page::Landing, Page::Cabinet] {
            assert!(
                !head(&site, page, Locale::Ru).contains("telegram-app"),
                "{page:?}"
            );
        }
    }

    #[test]
    fn название_сервиса_есть_в_разметке_каждой_страницы() {
        let mut named = settings();
        named.brand_name = "Тихий & \"VPN\"".into();
        for page in [
            Page::Landing,
            Page::Cabinet,
            Page::MiniApp,
            Page::Admin,
            Page::Other,
        ] {
            for locale in [Locale::Ru, Locale::En] {
                let tags = head(&named, page, locale);
                assert!(
                    tags.contains(
                        "<meta name=\"application-name\" content=\"Тихий &amp; &quot;VPN&quot;\">"
                    ),
                    "{page:?}: {tags}"
                );
            }
        }
    }

    #[test]
    fn кабинет_на_английском_получает_английский_заголовок_и_язык_страницы() {
        let cabinet = head(&settings(), Page::Cabinet, Locale::En);
        assert!(cabinet.contains("<title>Your account — Тихий VPN</title>"));
        let template = "<!doctype html>\n<html lang=\"ru\">\n<head></head><body></body></html>";
        assert!(with_lang(template.to_string(), Locale::En).contains("<html lang=\"en\">"));
        assert_eq!(with_lang(template.to_string(), Locale::Ru), template);
    }

    #[test]
    fn свой_фавикон_стоит_во_вкладке_на_всех_страницах() {
        let mut with_icon = settings();
        with_icon.favicon = Some(ImageMeta {
            tag: "fedcba9876543210".into(),
            kind: ImageKind::Ico,
            width: 48,
            height: 48,
            size: 9000,
        });
        for page in [
            Page::Landing,
            Page::Cabinet,
            Page::MiniApp,
            Page::Admin,
            Page::Other,
        ] {
            let tags = head(&with_icon, page, Locale::Ru);
            assert!(
                tags.contains(
                    "<link rel=\"icon\" type=\"image/x-icon\" href=\"/api/public/favicon/fedcba9876543210.ico\">"
                ),
                "{page:?}: {tags}"
            );
            assert!(
                tags.contains(
                    "<link rel=\"apple-touch-icon\" href=\"/api/public/logo/0123456789abcdef.png\">"
                ),
                "{page:?}: {tags}"
            );
        }
        with_icon.favicon = Some(ImageMeta {
            tag: "0011223344556677".into(),
            kind: ImageKind::Png,
            width: 512,
            height: 512,
            size: 20000,
        });
        let tags = head(&with_icon, Page::Admin, Locale::Ru);
        assert!(tags.contains(
            "<link rel=\"apple-touch-icon\" href=\"/api/public/favicon/0011223344556677.png\">"
        ));
        assert!(!tags.contains("og:"));
    }

    #[test]
    fn без_заголовка_в_шаблоне_теги_встают_перед_концом_head() {
        let html = inject(
            "<html><head></head><body></body></html>",
            "<title>X</title>",
        );
        assert_eq!(
            html,
            "<html><head><title>X</title></head><body></body></html>"
        );
        let html = inject("<p>без head</p>", "<title>X</title>");
        assert_eq!(html, "<title>X</title><p>без head</p>");
    }
}
