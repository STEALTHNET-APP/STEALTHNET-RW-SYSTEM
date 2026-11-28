use super::*;
use axum::extract::State;
use axum::response::IntoResponse;
use base64::Engine;
use sn_services::ext_modules::{Deliverer, package, signing};
use sn_services::updates::{BuildInfo, UpdateConfig};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(super) struct Folder(pub(super) PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn temp(prefix: &str) -> Folder {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    Folder(dir)
}

pub(super) fn helper_dir(helper: Option<Value>) -> Folder {
    let dir = temp("sn-modules-helper");
    std::fs::create_dir_all(dir.0.join("request")).unwrap();
    std::fs::create_dir_all(dir.0.join("status/modules")).unwrap();
    if let Some(helper) = helper {
        std::fs::write(dir.0.join("status/helper.json"), helper.to_string()).unwrap();
    }
    dir
}

pub(super) fn ready() -> Value {
    json!({ "schema": 2, "panel_updates": true, "modules": true, "blocked": null })
}

pub(super) fn configure(h: &Http, dir: Option<&Path>, why: Option<&str>, host: Option<&str>) {
    let mut config = UpdateConfig::new(BuildInfo::new(None, None));
    config.dir = dir.map(Path::to_path_buf);
    config.helper_why = why.map(str::to_string);
    config.modules_host = host.map(str::to_string);
    h.services().set_update_config(Some(config));
}

fn module(id: &str, version: &str, port: u16) -> Value {
    json!({
        "format": 1,
        "id": id,
        "name": "Автоустановка нод",
        "version": version,
        "author": "STEALTHNET",
        "page": { "path": "/", "title": "Установка нод", "permission": "infrastructure" },
        "settings": [{ "key": "remna_url", "type": "url", "label": "Адрес Remnawave" }],
        "run": {
            "port": port,
            "events_path": "/events",
            "health_path": "/healthz",
            "memory_mb": 128,
            "cpus": 0.5,
            "internet": true,
            "volumes": [{ "name": "data", "path": "/data" }]
        }
    })
}

pub(super) fn build(dir: &Path, manifest: &Value, image: &[u8]) -> Vec<u8> {
    let source = dir.join(format!("src-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("module.json"), manifest.to_string()).unwrap();
    let platform = package::host_platform().unwrap();
    let file = sn_services::ext_modules::manifest::PLATFORMS
        .iter()
        .find(|(p, _)| *p == platform)
        .unwrap()
        .1;
    std::fs::write(source.join(file), image).unwrap();
    let out = dir.join(format!("{}.zip", Uuid::new_v4()));
    package::pack(&source, None, Some(&out)).unwrap();
    std::fs::read(&out).unwrap()
}

async fn raw(
    h: &Http,
    method: Method,
    path: &str,
    token: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/zip")
        .body(Body::from(body))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

pub(super) async fn upload(h: &Http, token: &str, bytes: &[u8]) -> String {
    let half = bytes.len() / 2;
    let (status, first) = raw(
        h,
        Method::POST,
        "/api/admin/ext-modules/packages",
        token,
        bytes[..half].to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let id = first["id"].as_str().unwrap().to_string();
    let (status, second) = raw(
        h,
        Method::POST,
        &format!("/api/admin/ext-modules/packages/{id}"),
        token,
        bytes[half..].to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["size"], bytes.len() as u64);
    id
}

fn request_line(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("request/request.json")).unwrap_or_default()
}

pub(super) fn clear_request(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("request/request.json"));
}

async fn install(h: &Http, token: &str, upload: &str, trust: bool) -> (StatusCode, Value) {
    h.write(
        Method::POST,
        &format!("/api/admin/ext-modules/packages/{upload}/install"),
        token,
        json!({ "password": PASSWORD, "permissions": [], "events": [], "trust_unsigned": trust }),
    )
    .await
}

#[tokio::test]
async fn без_помощника_архив_не_ставится_и_причина_названа() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, info) = h.get("/api/admin/ext-modules/install-info", &owner).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert_eq!(info["packages"]["state"], "no_helper");
    assert_eq!(info["owner"], true);
    configure(&h, None, Some("no_root"), None);
    let (_, info) = h.get("/api/admin/ext-modules/install-info", &owner).await;
    assert!(
        info["reason"].as_str().unwrap().contains("без прав root"),
        "{info}"
    );
    for (file, state) in [
        (
            json!({ "schema": 1, "panel_updates": true, "blocked": null }),
            "outdated",
        ),
        (
            json!({ "schema": 1, "panel_updates": false, "blocked": null }),
            "helper_off",
        ),
        (
            json!({ "schema": 2, "panel_updates": true, "modules": false }),
            "modules_off",
        ),
        (ready(), "ready"),
    ] {
        let dir = helper_dir(Some(file));
        configure(&h, Some(&dir.0), None, None);
        let (_, info) = h.get("/api/admin/ext-modules/install-info", &owner).await;
        assert_eq!(info["packages"]["state"], state, "{info}");
        if state != "ready" {
            let (status, body) = raw(
                &h,
                Method::POST,
                "/api/admin/ext-modules/packages",
                &owner,
                b"PK".to_vec(),
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert_eq!(body["error"]["code"], "ext_modules.helper");
        }
    }
    h.db.cleanup().await;
}

#[tokio::test]
async fn архив_ставит_только_владелец_с_паролем_и_заявка_уходит_помощнику() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let admin = h.admin("admin@example.com", AdminRole::Admin).await;
    let dir = helper_dir(Some(ready()));
    configure(&h, Some(&dir.0), None, None);
    let work = temp("sn-modules-pack");
    let bytes = build(&work.0, &module("nodes", "1.0.0", 8095), b"image bytes");
    let (status, body) = raw(
        &h,
        Method::POST,
        "/api/admin/ext-modules/packages",
        &admin,
        bytes.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.owner_only");
    let upload = upload(&h, &owner, &bytes).await;
    let (status, checked) = h
        .write(
            Method::POST,
            &format!("/api/admin/ext-modules/packages/{upload}/check"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    let preview = &checked["preview"];
    assert_eq!(preview["id"], "nodes");
    assert_eq!(preview["run"]["memory_mb"], 128);
    assert_eq!(preview["run"]["cpus"], "0.5");
    assert_eq!(preview["run"]["internet"], true);
    assert_eq!(preview["run"]["page_path"], "/");
    assert_eq!(preview["package"]["signature"]["state"], "unsigned");
    assert_eq!(preview["page"]["path"], "/");
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/ext-modules/packages/{upload}/install"),
            &owner,
            json!({ "password": "не тот", "permissions": [], "events": [], "trust_unsigned": true }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.wrong_password");
    let (status, body) = install(&h, &owner, &upload, false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.untrusted");
    let (status, installed) = install(&h, &owner, &upload, true).await;
    assert_eq!(status, StatusCode::OK, "{installed}");
    assert_eq!(installed["module"]["managed"], true);
    assert_eq!(installed["runtime"]["state"], "installing");
    assert_eq!(installed["runtime"]["job"]["state"], "waiting");
    let line = request_line(&dir.0);
    let pattern = format!(
        "{{\"id\":\"{id}\",\"action\":\"module-install\",\"module\":\"nodes\",\"package\":\"{upload}\",\"requested_at\":\"",
        id = installed["runtime"]["job"]["id"].as_str().unwrap()
    );
    assert!(line.starts_with(&pattern), "{line}");
    assert!(line.ends_with(",\"admin_id\":1}\n"), "{line}");
    let job = installed["runtime"]["job"]["id"].as_str().unwrap();
    let cred = std::fs::read_to_string(dir.0.join(format!("request/{job}.cred"))).unwrap();
    let lines: Vec<&str> = cred.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with("snk_") && lines[0].len() == 52,
        "{cred}"
    );
    assert!(
        lines[1].starts_with("whsec_") && lines[1].len() == 50,
        "{cred}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.0.join(format!("request/{job}.cred")))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let (status, me) = h.get("/api/admin/ext-modules/me", lines[0]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["id"], "nodes");
    let other = upload_again(&h, &owner, &work.0).await;
    let (status, body) = install(&h, &owner, &other, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.busy");
    let mut conn = h.services().store().conn().await.unwrap();
    let audits = conn
        .audit()
        .recent_by_action("ext_module.package_installed", 5)
        .await
        .unwrap();
    assert_eq!(audits.len(), 1);
    drop(conn);
    h.db.cleanup().await;
}

async fn upload_again(h: &Http, token: &str, work: &Path) -> String {
    let bytes = build(work, &module("nodes", "1.0.1", 8095), b"other image");
    upload(h, token, &bytes).await
}

fn write_status(dir: &Path, id: &str, job: &str, state: &str, container: &str) {
    std::fs::write(
        dir.join(format!("status/modules/{id}.json")),
        json!({
            "schema": 1,
            "module": id,
            "job": { "id": job, "action": "module-install", "state": state, "steps": [
                { "key": "check", "state": "done", "seconds": 1 },
                { "key": "load", "state": "done", "seconds": 2 },
                { "key": "start", "state": "done", "seconds": 1 },
                { "key": "verify", "state": state, "seconds": 3 }
            ], "error": null, "message": null, "started_at": "2026-11-16T14:52:00Z", "finished_at": "2026-11-16T14:52:07Z" },
            "container": { "state": container, "started_at": "2026-11-16T14:52:05.123456789Z", "restarts": 0,
                "image": "stealthnet-module/nodes:0123456789ab", "memory_bytes": 64172851, "data_bytes": 4096,
                "checked_at": "2026-11-16T14:52:07Z" }
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn состояние_обновление_и_удаление_модуля_из_архива() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let dir = helper_dir(Some(ready()));
    configure(&h, Some(&dir.0), None, None);
    let work = temp("sn-modules-pack");
    let first = build(&work.0, &module("nodes", "1.0.0", 8095), b"image one");
    let upload_one = upload(&h, &owner, &first).await;
    let (status, installed) = install(&h, &owner, &upload_one, true).await;
    assert_eq!(status, StatusCode::OK, "{installed}");
    let job = installed["runtime"]["job"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    clear_request(&dir.0);
    write_status(&dir.0, "nodes", &job, "done", "running");
    let (status, runtime) = h.get("/api/admin/ext-modules/nodes/runtime", &owner).await;
    assert_eq!(status, StatusCode::OK, "{runtime}");
    assert_eq!(runtime["state"], "running", "{runtime}");
    assert_eq!(runtime["job"]["state"], "done");
    assert_eq!(runtime["container"]["memory_bytes"], 64172851);
    for (ok, at, state) in [
        (false, "2026-11-16T14:52:30Z", "unhealthy"),
        (true, "2026-11-16T14:52:40Z", "running"),
    ] {
        {
            let mut conn = h.services().store().conn().await.unwrap();
            sn_services::ext_modules::store::set_health(
                &mut conn,
                "nodes",
                &json!({ "ok": ok, "at": at, "error": null, "since": null }),
            )
            .await
            .unwrap();
        }
        let (_, runtime) = h.get("/api/admin/ext-modules/nodes/runtime", &owner).await;
        assert_eq!(runtime["state"], state, "{runtime}");
    }
    let (_, list) = h.get("/api/admin/ext-modules", &owner).await;
    assert_eq!(list[0]["managed"], true);
    assert_eq!(list[0]["state"], "running");
    let (status, restarted) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/nodes/restart",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restarted}");
    assert_eq!(restarted["state"], "starting");
    assert!(request_line(&dir.0).contains("\"action\":\"module-restart\",\"module\":\"nodes\""));
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/nodes/restart",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.busy");
    clear_request(&dir.0);
    std::fs::write(
        dir.0.join("status/modules/nodes.log"),
        "запуск\n\u{1b}[31mкрасная\u{1b}[0m строка\n",
    )
    .unwrap();
    let (status, logs) = h.get("/api/admin/ext-modules/nodes/logs", &owner).await;
    assert_eq!(status, StatusCode::OK, "{logs}");
    assert_eq!(logs["lines"], json!(["запуск", "красная строка"]));
    let second = build(&work.0, &module("nodes", "1.1.0", 8095), b"image two");
    let upload_two = upload(&h, &owner, &second).await;
    let (status, checked) = h
        .write(
            Method::POST,
            &format!("/api/admin/ext-modules/packages/{upload_two}/check"),
            &owner,
            json!({ "update": "nodes" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    assert_eq!(checked["preview"]["installed"]["version"], "1.0.0");
    assert_eq!(checked["preview"]["previous_run"]["memory_mb"], 128);
    let (status, updated) = h
        .write(
            Method::POST,
            &format!("/api/admin/ext-modules/packages/{upload_two}/install"),
            &owner,
            json!({ "password": PASSWORD, "permissions": [], "events": [], "trust_unsigned": true, "update": "nodes" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["module"]["version"], "1.0.0");
    assert_eq!(updated["runtime"]["state"], "updating");
    let line = request_line(&dir.0);
    assert!(
        line.contains(&format!(
            "\"action\":\"module-update\",\"module\":\"nodes\",\"package\":\"{upload_two}\""
        )),
        "{line}"
    );
    let update_job = updated["runtime"]["job"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!dir.0.join(format!("request/{update_job}.cred")).exists());
    clear_request(&dir.0);
    write_status(&dir.0, "nodes", &update_job, "running", "running");
    let (_, card) = h.get("/api/admin/ext-modules/nodes", &owner).await;
    assert_eq!(card["version"], "1.0.0");
    write_status(&dir.0, "nodes", &update_job, "done", "running");
    let (_, card) = h.get("/api/admin/ext-modules/nodes", &owner).await;
    assert_eq!(card["version"], "1.1.0", "{card}");
    let (_, runtime) = h.get("/api/admin/ext-modules/nodes/runtime", &owner).await;
    assert!(runtime["package"]["pending"].is_null(), "{runtime}");
    let third = build(&work.0, &module("nodes", "1.2.0", 8095), b"image three");
    let upload_three = upload(&h, &owner, &third).await;
    let (status, updated) = h
        .write(
            Method::POST,
            &format!("/api/admin/ext-modules/packages/{upload_three}/install"),
            &owner,
            json!({ "password": PASSWORD, "permissions": [], "events": [], "trust_unsigned": true, "update": "nodes" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let failed_job = updated["runtime"]["job"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    clear_request(&dir.0);
    write_status(&dir.0, "nodes", &failed_job, "rolled_back", "running");
    let (_, card) = h.get("/api/admin/ext-modules/nodes", &owner).await;
    assert_eq!(card["version"], "1.1.0", "{card}");
    let (_, runtime) = h.get("/api/admin/ext-modules/nodes/runtime", &owner).await;
    assert!(runtime["package"]["pending"].is_null(), "{runtime}");
    assert_eq!(runtime["job"]["state"], "rolled_back");
    let (status, _, body) = h
        .send(
            Method::DELETE,
            "/api/admin/ext-modules/nodes",
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.password");
    let key = {
        let mut conn = h.services().store().conn().await.unwrap();
        sn_services::ext_modules::store::get(&mut conn, "nodes")
            .await
            .unwrap()
            .unwrap()
            .api_key_id
            .unwrap()
    };
    let (status, _, body) = h
        .send(
            Method::DELETE,
            "/api/admin/ext-modules/nodes",
            Some(&owner),
            Some(json!({ "password": PASSWORD, "erase_data": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let line = request_line(&dir.0);
    assert!(
        line.contains("\"action\":\"module-remove\",\"module\":\"nodes\",\"data\":\"erase\""),
        "{line}"
    );
    let (status, _) = h.get("/api/admin/ext-modules/nodes", &owner).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut conn = h.services().store().conn().await.unwrap();
    let revoked: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM api_keys WHERE id = $1")
            .bind(key)
            .fetch_one(conn.conn())
            .await
            .unwrap();
    assert!(revoked.is_some());
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn файл_описания_не_заменяет_модуль_из_архива() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let dir = helper_dir(Some(ready()));
    configure(&h, Some(&dir.0), None, None);
    let work = temp("sn-modules-pack");
    let bytes = build(&work.0, &module("nodes", "1.0.0", 8095), b"image");
    let upload = upload(&h, &owner, &bytes).await;
    let (status, body) = install(&h, &owner, &upload, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let manifest = json!({
        "format": 1, "id": "nodes", "name": "Подмена", "version": "9.9.9", "author": "кто-то",
        "webhook": "https://evil.example.com/events"
    })
    .to_string();
    let (status, preview) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/preview",
            &owner,
            json!({ "manifest": manifest }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(preview["preview"].is_null());
    assert!(
        preview["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("установлен из архива")
    );
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules",
            &owner,
            json!({ "manifest": manifest, "permissions": [], "events": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.managed");
    h.db.cleanup().await;
}

type Hit = (String, HeaderMap, Vec<u8>);

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Hit>>>);

async fn fake_module(
    State(seen): State<Seen>,
    method: Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    seen.0
        .lock()
        .unwrap()
        .push((format!("{method} {uri}"), headers, body.to_vec()));
    match uri.path() {
        "/go" => (StatusCode::FOUND, [(header::LOCATION, "/login")]).into_response(),
        "/away" => (
            StatusCode::FOUND,
            [(header::LOCATION, "https://evil.example/")],
        )
            .into_response(),
        "/events" => (StatusCode::OK, "принято").into_response(),
        _ => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::SET_COOKIE, "sn_admin=stolen; Path=/api"),
                (header::X_FRAME_OPTIONS, "DENY"),
                (header::CONTENT_SECURITY_POLICY, "default-src 'self'"),
            ],
            "страница модуля",
        )
            .into_response(),
    }
}

async fn spawn_module() -> (u16, Seen) {
    let seen = Seen::default();
    let app = axum::Router::new()
        .fallback(fake_module)
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, seen)
}

pub(super) async fn page_get(
    h: &Http,
    method: Method,
    path: &str,
    cookie: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, "Bearer module-page-token");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let response = router(h.state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn страница_модуля_из_архива_открывается_через_прокси_панели() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (port, seen) = spawn_module().await;
    let dir = helper_dir(Some(ready()));
    configure(&h, Some(&dir.0), None, Some("127.0.0.1"));
    let work = temp("sn-modules-pack");
    let mut manifest = module("nodes", "1.0.0", port);
    manifest["page"]["permission"] = json!("modules");
    let bytes = build(&work.0, &manifest, b"image");
    let upload = upload(&h, &owner, &bytes).await;
    let (status, body) = install(&h, &owner, &upload, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    clear_request(&dir.0);
    let (status, _, token) = h
        .send(
            Method::POST,
            "/api/admin/ext-modules/nodes/page-token",
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{token}");
    assert_eq!(token["proxied"], true);
    let url = token["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/m/nodes/"), "{url}");
    assert!(url.contains("/?sn_token="), "{url}");
    let prefix = url
        .split('?')
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_string();
    let (status, headers, body) = page_get(&h, Method::GET, &url, Some("sn_admin=секрет")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, "страница модуля");
    let csp: Vec<String> = headers
        .get_all(header::CONTENT_SECURITY_POLICY)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(csp.contains(&"default-src 'self'".to_string()), "{csp:?}");
    assert!(
        csp.iter()
            .any(|v| v.starts_with("sandbox allow-scripts") && v.contains("frame-ancestors 'self'")),
        "{csp:?}"
    );
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(headers.get(header::X_FRAME_OPTIONS).is_none());
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
    {
        let calls = seen.0.lock().unwrap();
        let (line, sent, _) = calls.last().unwrap();
        assert!(line.starts_with("GET /?sn_token="), "{line}");
        assert!(sent.get(header::COOKIE).is_none(), "{sent:?}");
        assert_eq!(
            sent.get(header::AUTHORIZATION).unwrap(),
            "Bearer module-page-token"
        );
        assert_eq!(
            sent.get("x-forwarded-prefix").unwrap().to_str().unwrap(),
            prefix
        );
        let identity = sent.get("x-stealthnet-admin").unwrap().to_str().unwrap();
        let payload = identity.split('.').nth(1).unwrap();
        let claims: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["aud"], "module:nodes");
        assert_eq!(claims["email"], "owner@example.com");
    }
    let (status, headers, _) = page_get(&h, Method::GET, &format!("{prefix}/go"), None).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).unwrap().to_str().unwrap(),
        format!("{prefix}/login")
    );
    let (status, _, _) = page_get(&h, Method::GET, &format!("{prefix}/away"), None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let (status, headers, _) =
        page_get(&h, Method::OPTIONS, &format!("{prefix}/api/state"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    let (status, _, _) = page_get(
        &h,
        Method::GET,
        "/m/nodes/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAA/",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let cap = prefix.trim_start_matches("/m/nodes/");
    let (status, _, _) = page_get(&h, Method::GET, &format!("/m/other/{cap}/"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, headers, _) = page_get(&h, Method::GET, &prefix, None).await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        headers.get(header::LOCATION).unwrap().to_str().unwrap(),
        format!("{prefix}/")
    );
    let (status, ping) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/nodes/ping",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{ping}");
    assert_eq!(ping["ok"], true, "{ping}");
    {
        let calls = seen.0.lock().unwrap();
        let (line, sent, body) = calls.last().unwrap();
        assert_eq!(line, "POST /events");
        assert!(
            sent.get("webhook-signature")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("v1,")
        );
        assert!(String::from_utf8_lossy(body).contains("\"type\":\"ping\""));
    }
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/ext-modules/nodes",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(request_line(&dir.0).contains("\"action\":\"module-stop\""));
    let (status, _, _) = page_get(&h, Method::GET, &url, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    h.db.cleanup().await;
}

async fn save_address(h: &Http, token: &str, address: &str, queued: bool) {
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/ext-modules/nodes/config",
            token,
            json!({ "config": { "remna_url": address } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["remna_url"], address, "{body}");
    assert_eq!(body["event_queued"], queued, "{body}");
}

async fn takes_events(h: &Http, token: &str) -> Value {
    let (status, card) = h.get("/api/admin/ext-modules/nodes", token).await;
    assert_eq!(status, StatusCode::OK, "{card}");
    card["takes_events"].clone()
}

async fn deliveries(h: &Http) -> Vec<(String, Value, String)> {
    let mut conn = h.services().store().conn().await.unwrap();
    sqlx::query_as("SELECT kind, payload, status FROM ext_module_deliveries ORDER BY id")
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn сохранение_настроек_доходит_до_модуля_из_архива_событием() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (port, seen) = spawn_module().await;
    let dir = helper_dir(Some(ready()));
    configure(&h, Some(&dir.0), None, Some("127.0.0.1"));
    let work = temp("sn-modules-pack");
    let bytes = build(&work.0, &module("nodes", "1.0.0", port), b"image");
    let archive = upload(&h, &owner, &bytes).await;
    let (status, installed) = install(&h, &owner, &archive, true).await;
    assert_eq!(status, StatusCode::OK, "{installed}");
    let job = installed["runtime"]["job"]["id"].as_str().unwrap();
    let cred = std::fs::read_to_string(dir.0.join(format!("request/{job}.cred"))).unwrap();
    let secret = cred.lines().nth(1).unwrap().to_string();
    clear_request(&dir.0);
    assert_eq!(takes_events(&h, &owner).await, true);
    save_address(&h, &owner, "https://remna.example.com", true).await;
    let changed = |status: &str| {
        (
            "settings_changed".to_string(),
            json!({ "keys": ["remna_url"] }),
            status.to_string(),
        )
    };
    assert_eq!(deliveries(&h).await, [changed("pending")]);
    let deliverer = Deliverer::new(h.services().clone());
    assert_eq!(deliverer.round().await.unwrap(), 1);
    {
        let calls = seen.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (line, sent, body) = &calls[0];
        assert_eq!(line, "POST /events");
        let header = |name: &str| sent.get(name).unwrap().to_str().unwrap().to_string();
        let id = header("webhook-id");
        assert!(id.starts_with("msg_"), "{id}");
        assert_eq!(
            header("webhook-signature"),
            signing::signature(
                &signing::key(&secret).unwrap(),
                &id,
                header("webhook-timestamp").parse().unwrap(),
                body
            )
            .unwrap()
        );
        let event: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(event["id"], id.as_str());
        assert_eq!(event["type"], "settings_changed");
        assert_eq!(event["module"], "nodes");
        assert_eq!(event["data"], json!({ "keys": ["remna_url"] }));
    }
    assert_eq!(deliveries(&h).await, [changed("done")]);
    let (status, journal) = h
        .get("/api/admin/ext-modules/nodes/deliveries", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{journal}");
    let attempt = &journal["attempts"][0];
    assert_eq!(attempt["kind"], "settings_changed", "{journal}");
    assert_eq!(attempt["title"], "Настройки изменены");
    assert_eq!(attempt["status"], "done");
    assert_eq!(attempt["http_status"], 200);
    assert_eq!(attempt["response"], "принято");
    save_address(&h, &owner, "https://remna.example.com", false).await;
    assert_eq!(deliveries(&h).await, [changed("done")]);
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/ext-modules/nodes",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    clear_request(&dir.0);
    save_address(&h, &owner, "https://new.example.com", false).await;
    assert_eq!(deliveries(&h).await, [changed("done")]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn модулю_из_архива_без_пути_событий_ничего_не_отправляется() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (port, seen) = spawn_module().await;
    let dir = helper_dir(Some(ready()));
    configure(&h, Some(&dir.0), None, Some("127.0.0.1"));
    let work = temp("sn-modules-pack");
    let mut manifest = module("nodes", "1.0.0", port);
    manifest["run"]
        .as_object_mut()
        .unwrap()
        .remove("events_path");
    let bytes = build(&work.0, &manifest, b"image");
    let archive = upload(&h, &owner, &bytes).await;
    let (status, body) = install(&h, &owner, &archive, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    clear_request(&dir.0);
    assert_eq!(takes_events(&h, &owner).await, false);
    save_address(&h, &owner, "https://remna.example.com", false).await;
    assert_eq!(deliveries(&h).await, []);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/nodes/ping",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "ext_modules.no_webhook");
    assert!(seen.0.lock().unwrap().is_empty());
    h.db.cleanup().await;
}
