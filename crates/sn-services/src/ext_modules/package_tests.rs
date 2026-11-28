use super::*;
use crate::ext_modules::manifest::{parse, parse_package};
use std::io::Cursor;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sn-package-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tar_with(entries: &[(&str, &[u8])], links: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, name, *data).unwrap();
    }
    for (name, target) in links {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(tar::EntryType::Symlink);
        builder.append_link(&mut header, name, target).unwrap();
    }
    builder.into_inner().unwrap()
}

const LONG: &str = "blobs/sha256/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn fake_image() -> Vec<u8> {
    tar_with(
        &[
            (
                "manifest.json",
                br#"[{"Config":"blobs/sha256/aaa","RepoTags":["postgres:16-alpine"],"Layers":["blobs/sha256/bbb"]}]"#,
            ),
            (
                "index.json",
                br#"{"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:ccc","size":10,"annotations":{"io.containerd.image.name":"docker.io/library/postgres:16-alpine","org.opencontainers.image.ref.name":"16-alpine","other":"keep"}}]}"#,
            ),
            ("repositories", br#"{"postgres":{"16-alpine":"aaa"}}"#),
            ("oci-layout", br#"{"imageLayoutVersion":"1.0.0"}"#),
            ("blobs/sha256/aaa", br#"{"architecture":"amd64"}"#),
            ("blobs/sha256/bbb", b"layer bytes"),
            (LONG, b"long name"),
        ],
        &[("legacy/layer.tar", "../blobs/sha256/bbb")],
    )
}

fn sha(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn module_json(images: &[(&str, &str, &[u8])]) -> String {
    let images: Map<String, Value> = images
        .iter()
        .map(|(platform, file, data)| {
            (
                platform.to_string(),
                serde_json::json!({ "file": file, "sha256": sha(data), "bytes": data.len() }),
            )
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "format": 1,
        "id": "nodes",
        "name": "Автоустановка нод",
        "version": "1.1.0",
        "author": "STEALTHNET",
        "page": { "path": "/", "title": "Установка нод", "permission": "infrastructure" },
        "run": {
            "port": 8095,
            "events_path": "/events",
            "health_path": "/healthz",
            "memory_mb": 128,
            "cpus": 0.5,
            "internet": true,
            "volumes": [{ "name": "data", "path": "/data" }],
            "images": images
        }
    }))
    .unwrap()
}

fn zip_with(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap();
}

fn key_pair() -> (Ed25519KeyPair, String) {
    let rng = ring::rand::SystemRandom::new();
    let doc = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    let pair = Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap();
    let public = STANDARD.encode(pair.public_key().as_ref());
    (pair, public)
}

fn messages(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|p| format!("{}: {}", p.path, p.message))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn архив_без_подписи_проходит_проверку() {
    let dir = temp_dir();
    let image = fake_image();
    let json = module_json(&[("linux/amd64", "image-linux-amd64.tar", &image)]);
    let path = dir.join("m.zip");
    zip_with(
        &path,
        &[
            ("module.json", json.as_bytes()),
            ("image-linux-amd64.tar", &image),
        ],
    );
    let inspected = inspect(&path, "linux/amd64", TRUSTED).unwrap();
    assert_eq!(inspected.signature, Signature::Unsigned);
    assert_eq!(inspected.manifest.id, "nodes");
    assert_eq!(inspected.image.bytes, image.len() as u64);
    assert_eq!(inspected.platforms, vec!["linux/amd64".to_string()]);
    let run = inspected.manifest.run.unwrap();
    assert_eq!(run.cpu_tenths, 5);
    assert_eq!(run.memory_mb, 128);
    assert!(run.internet);
    assert_eq!(inspected.manifest.page.unwrap().path.as_deref(), Some("/"));
}

#[test]
fn подпись_проверяется_и_узнаётся_ключ() {
    let dir = temp_dir();
    let image = fake_image();
    let json = module_json(&[("linux/amd64", "image-linux-amd64.tar", &image)]);
    let (pair, public) = key_pair();
    let signature = sign(&pair, json.as_bytes());
    let path = dir.join("signed.zip");
    zip_with(
        &path,
        &[
            ("module.json", json.as_bytes()),
            ("module.sig", signature.as_bytes()),
            ("image-linux-amd64.tar", &image),
        ],
    );
    let known = inspect(&path, "linux/amd64", &[("Тест", public.as_str())]).unwrap();
    let print = fingerprint(pair.public_key().as_ref());
    assert_eq!(
        known.signature,
        Signature::Verified {
            signer: "Тест".into(),
            fingerprint: print.clone()
        }
    );
    assert_eq!(print.len(), 19);
    let unknown = inspect(&path, "linux/amd64", TRUSTED).unwrap();
    assert_eq!(unknown.signature, Signature::Unknown { fingerprint: print });
}

#[test]
fn описание_изменённое_после_подписи_не_проходит() {
    let dir = temp_dir();
    let image = fake_image();
    let json = module_json(&[("linux/amd64", "image-linux-amd64.tar", &image)]);
    let (pair, public) = key_pair();
    let signature = sign(&pair, json.as_bytes());
    let changed = json.replace("\"memory_mb\": 128", "\"memory_mb\": 2048");
    assert_ne!(changed, json);
    let path = dir.join("changed.zip");
    zip_with(
        &path,
        &[
            ("module.json", changed.as_bytes()),
            ("module.sig", signature.as_bytes()),
            ("image-linux-amd64.tar", &image),
        ],
    );
    let err = inspect(&path, "linux/amd64", &[("Тест", public.as_str())]).unwrap_err();
    assert!(messages(&err).contains("подпись не совпадает"), "{err:?}");
}

#[test]
fn пути_папки_ссылки_и_лишние_файлы_отклоняются() {
    let dir = temp_dir();
    let image = fake_image();
    let json = module_json(&[("linux/amd64", "image-linux-amd64.tar", &image)]);
    let path = dir.join("evil.zip");
    let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("module.json", options).unwrap();
    writer.write_all(json.as_bytes()).unwrap();
    writer.start_file("../../etc/cron.d/evil", options).unwrap();
    writer.write_all(b"* * * * * root sh").unwrap();
    writer.add_directory("data/", options).unwrap();
    writer
        .add_symlink("image-linux-arm64.tar", "/etc/shadow", options)
        .unwrap();
    writer.finish().unwrap();
    let err = inspect(&path, "linux/amd64", TRUSTED).unwrap_err();
    let text = messages(&err);
    assert!(
        text.contains("../../etc/cron.d/evil: лишний файл"),
        "{text}"
    );
    assert!(
        text.contains("data/: в архиве модуля не бывает папок"),
        "{text}"
    );
    assert!(text.contains("image-linux-arm64.tar: ссылки"), "{text}");
}

#[test]
fn слишком_много_файлов_отклоняется_сразу() {
    let dir = temp_dir();
    let path = dir.join("many.zip");
    let names: Vec<String> = (0..6).map(|i| format!("f{i}")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    zip_with(&path, &entries);
    let err = inspect(&path, "linux/amd64", TRUSTED).unwrap_err();
    assert!(messages(&err).contains("в архиве 6 файлов"), "{err:?}");
}

#[test]
fn не_zip_и_пустой_файл_отклоняются() {
    let dir = temp_dir();
    let json = dir.join("module.json");
    std::fs::write(&json, b"{\"format\":1}").unwrap();
    assert!(
        messages(&inspect(&json, "linux/amd64", TRUSTED).unwrap_err()).contains("не zip-архив")
    );
    let empty = dir.join("empty.zip");
    std::fs::write(&empty, b"").unwrap();
    assert!(messages(&inspect(&empty, "linux/amd64", TRUSTED).unwrap_err()).contains("пустой"));
}

#[test]
fn чужая_контрольная_сумма_и_размер_отклоняются() {
    let dir = temp_dir();
    let image = fake_image();
    let other = b"other image".to_vec();
    let json = module_json(&[("linux/amd64", "image-linux-amd64.tar", &other)]);
    let path = dir.join("sum.zip");
    zip_with(
        &path,
        &[
            ("module.json", json.as_bytes()),
            ("image-linux-amd64.tar", &image),
        ],
    );
    let err = inspect(&path, "linux/amd64", TRUSTED).unwrap_err();
    assert!(messages(&err).contains("занимает"), "{err:?}");
    let same_size: Vec<u8> = image.iter().map(|b| b.wrapping_add(1)).collect();
    let json = module_json(&[("linux/amd64", "image-linux-amd64.tar", &same_size)]);
    let path = dir.join("sum2.zip");
    zip_with(
        &path,
        &[
            ("module.json", json.as_bytes()),
            ("image-linux-amd64.tar", &image),
        ],
    );
    let err = inspect(&path, "linux/amd64", TRUSTED).unwrap_err();
    assert!(messages(&err).contains("контрольная сумма"), "{err:?}");
}

#[test]
fn без_сборки_для_процессора_сервера_не_ставится() {
    let dir = temp_dir();
    let image = fake_image();
    let json = module_json(&[("linux/arm64", "image-linux-arm64.tar", &image)]);
    let path = dir.join("arm.zip");
    zip_with(
        &path,
        &[
            ("module.json", json.as_bytes()),
            ("image-linux-arm64.tar", &image),
        ],
    );
    let err = inspect(&path, "linux/amd64", TRUSTED).unwrap_err();
    assert!(
        messages(&err).contains("нет сборки для процессора этого сервера (x86_64)"),
        "{err:?}"
    );
    assert!(inspect(&path, "linux/arm64", TRUSTED).is_ok());
}

#[test]
fn неописанный_образ_отклоняется() {
    let dir = temp_dir();
    let image = fake_image();
    let json = module_json(&[("linux/amd64", "image-linux-amd64.tar", &image)]);
    let path = dir.join("extra.zip");
    zip_with(
        &path,
        &[
            ("module.json", json.as_bytes()),
            ("image-linux-amd64.tar", &image),
            ("image-linux-arm64.tar", &image),
        ],
    );
    let err = inspect(&path, "linux/amd64", TRUSTED).unwrap_err();
    assert!(messages(&err).contains("образ не описан"), "{err:?}");
}

fn read_tar(data: &[u8]) -> Vec<(String, tar::EntryType, Vec<u8>, Option<String>)> {
    let mut archive = tar::Archive::new(Cursor::new(data));
    archive
        .entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let name = e.path().unwrap().to_string_lossy().into_owned();
            let kind = e.header().entry_type();
            let link = e
                .link_name()
                .unwrap()
                .map(|l| l.to_string_lossy().into_owned());
            let mut data = Vec::new();
            e.read_to_end(&mut data).unwrap();
            (name, kind, data, link)
        })
        .collect()
}

#[test]
fn образ_получает_только_свою_метку() {
    let mut out = Vec::new();
    normalize(
        Cursor::new(fake_image()),
        &mut out,
        "stealthnet-module/nodes:0123456789ab",
    )
    .unwrap();
    let entries = read_tar(&out);
    let names: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
    assert!(!names.contains(&"repositories"), "{names:?}");
    assert!(names.contains(&LONG), "{names:?}");
    let manifest: Value =
        serde_json::from_slice(&entries.iter().find(|e| e.0 == "manifest.json").unwrap().2)
            .unwrap();
    assert_eq!(
        manifest[0]["RepoTags"],
        serde_json::json!(["stealthnet-module/nodes:0123456789ab"])
    );
    assert_eq!(
        manifest[0]["Layers"],
        serde_json::json!(["blobs/sha256/bbb"])
    );
    let index: Value =
        serde_json::from_slice(&entries.iter().find(|e| e.0 == "index.json").unwrap().2).unwrap();
    let annotations = &index["manifests"][0]["annotations"];
    assert_eq!(
        annotations["io.containerd.image.name"],
        "docker.io/stealthnet-module/nodes:0123456789ab"
    );
    assert_eq!(
        annotations["org.opencontainers.image.ref.name"],
        "0123456789ab"
    );
    assert_eq!(annotations["other"], "keep");
    let link = entries.iter().find(|e| e.0 == "legacy/layer.tar").unwrap();
    assert!(link.1.is_symlink());
    assert_eq!(link.3.as_deref(), Some("../blobs/sha256/bbb"));
    let layer = entries.iter().find(|e| e.0 == "blobs/sha256/bbb").unwrap();
    assert_eq!(layer.2, b"layer bytes");
}

#[test]
fn два_образа_в_одном_архиве_не_принимаются() {
    let image = tar_with(
        &[(
            "manifest.json",
            br#"[{"Config":"a","RepoTags":["x:1"],"Layers":[]},{"Config":"b","RepoTags":["postgres:16-alpine"],"Layers":[]}]"#,
        )],
        &[],
    );
    let err = normalize(Cursor::new(image), Vec::new(), "stealthnet-module/x:1").unwrap_err();
    assert!(err.contains("ровно один образ"), "{err}");
    let empty = tar_with(&[("blobs/sha256/aaa", b"{}")], &[]);
    let err = normalize(Cursor::new(empty), Vec::new(), "stealthnet-module/x:1").unwrap_err();
    assert!(err.contains("нет manifest.json"), "{err}");
}

#[test]
fn сборка_подпись_и_распаковка_архива() {
    let dir = temp_dir();
    let source = dir.join("src");
    std::fs::create_dir_all(&source).unwrap();
    let mut json: Value = serde_json::from_str(&module_json(&[])).unwrap();
    json["run"].as_object_mut().unwrap().remove("images");
    std::fs::write(
        source.join("module.json"),
        serde_json::to_vec_pretty(&json).unwrap(),
    )
    .unwrap();
    let image = fake_image();
    std::fs::write(source.join("image-linux-amd64.tar"), &image).unwrap();
    std::fs::write(source.join("image-linux-arm64.tar"), &image).unwrap();
    let key = dir.join("signing.key");
    let (public, print) = keygen(&key).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(keygen(&key).is_err());
    let out = dir.join("nodes.zip");
    let packed = pack(&source, Some(&key), Some(&out)).unwrap();
    assert_eq!(packed.fingerprint.as_deref(), Some(print.as_str()));
    assert_eq!(
        packed.platforms,
        vec!["linux/amd64".to_string(), "linux/arm64".to_string()]
    );
    assert!(pack(&source, Some(&key), Some(&out)).is_err());
    let inspected = inspect(&out, "linux/arm64", &[("STEALTHNET", public.as_str())]).unwrap();
    assert!(inspected.signature.verified());
    let target = dir.join("out");
    std::fs::create_dir_all(&target).unwrap();
    let spec = unpack(&out, &target, "linux/amd64").unwrap();
    let tag = image_tag("nodes", &sha(&image));
    assert_eq!(spec.image, tag);
    assert_eq!(
        std::fs::read_to_string(target.join(SPEC_FILE)).unwrap(),
        format!(
            "module=nodes\nimage={tag}\nport=8095\nhealth=/healthz\nmemory=128\ncpus=0.5\ninternet=1\nvolume=data:/data\n"
        )
    );
    let loaded = read_tar(&std::fs::read(target.join(IMAGE_OUT)).unwrap());
    let manifest: Value =
        serde_json::from_slice(&loaded.iter().find(|e| e.0 == "manifest.json").unwrap().2).unwrap();
    assert_eq!(manifest[0]["RepoTags"], serde_json::json!([tag]));
    assert!(unpack(&out, &target, "linux/amd64").is_err());
}

#[test]
fn раздел_run_только_в_архиве() {
    let image = fake_image();
    let package = module_json(&[("linux/amd64", "image-linux-amd64.tar", &image)]);
    let err = parse(&package).unwrap_err();
    let text = messages(&err);
    assert!(
        text.contains("run: раздел run бывает только в архиве"),
        "{text}"
    );
    assert!(
        text.contains("page.path: page.path бывает только"),
        "{text}"
    );
    let mut value: Value = serde_json::from_str(&package).unwrap();
    value["webhook"] = Value::String("https://example.com/events".into());
    value["page"]["url"] = Value::String("https://example.com/".into());
    let err = parse_package(&value.to_string()).unwrap_err();
    let text = messages(&err);
    assert!(
        text.contains("webhook: в архиве модуля адреса нет"),
        "{text}"
    );
    assert!(
        text.contains("page.url: в архиве модуля адреса страницы нет"),
        "{text}"
    );
    let mut value: Value = serde_json::from_str(&package).unwrap();
    value.as_object_mut().unwrap().remove("run");
    let err = parse_package(&value.to_string()).unwrap_err();
    assert!(messages(&err).contains("нужен раздел run"));
}

#[test]
fn раздел_run_проверяется_по_полям() {
    let image = fake_image();
    let base: Value = serde_json::from_str(&module_json(&[(
        "linux/amd64",
        "image-linux-amd64.tar",
        &image,
    )]))
    .unwrap();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "port",
            serde_json::json!(80),
            "run.port: ожидается целое число от 1024",
        ),
        (
            "port",
            serde_json::json!("8095"),
            "run.port: ожидается число",
        ),
        ("cpus", serde_json::json!(0.55), "run.cpus"),
        ("cpus", serde_json::json!(3), "run.cpus"),
        ("memory_mb", serde_json::json!(4096), "run.memory_mb"),
        (
            "health_path",
            serde_json::json!("healthz"),
            "run.health_path: путь внутри модуля начинается с /",
        ),
        (
            "health_path",
            serde_json::json!("/../x"),
            "run.health_path: в пути не бывает",
        ),
        (
            "events_path",
            serde_json::json!("/e v"),
            "run.events_path: в пути только",
        ),
        (
            "volumes",
            serde_json::json!([{ "name": "data", "path": "/" }]),
            "run.volumes[0].path: нельзя /",
        ),
        (
            "volumes",
            serde_json::json!([{ "name": "data", "path": "/run/x" }]),
            "run.volumes[0].path: нельзя",
        ),
        (
            "volumes",
            serde_json::json!([{ "name": "Data", "path": "/data" }]),
            "run.volumes[0].name",
        ),
        (
            "volumes",
            serde_json::json!([{ "name": "a", "path": "/data" }, { "name": "b", "path": "/data/x" }]),
            "run.volumes[1].path: папки данных не могут совпадать",
        ),
        (
            "volumes",
            serde_json::json!([{ "name": "a", "path": "/a" }, { "name": "b", "path": "/b" }, { "name": "c", "path": "/c" }, { "name": "d", "path": "/d" }]),
            "run.volumes: не больше 3",
        ),
        (
            "images",
            serde_json::json!({ "linux/386": { "file": "x", "sha256": "0", "bytes": 1 } }),
            "run.images.linux/386: неизвестная платформа",
        ),
        (
            "images",
            serde_json::json!({ "linux/amd64": { "file": "image.tar", "sha256": sha(&image), "bytes": 1 } }),
            "run.images.linux/amd64.file: файл образа для linux/amd64 называется image-linux-amd64.tar",
        ),
        ("extra", serde_json::json!(1), "run.extra: неизвестное поле"),
    ];
    for (field, value, expected) in cases {
        let mut manifest = base.clone();
        manifest["run"][field] = value;
        let err = parse_package(&manifest.to_string()).unwrap_err();
        let text = messages(&err);
        assert!(text.contains(expected), "{field}: {text}");
    }
    let mut manifest = base.clone();
    manifest["events"] = serde_json::json!(["client_registered"]);
    manifest["permissions"] = serde_json::json!(["clients_read"]);
    manifest["run"]
        .as_object_mut()
        .unwrap()
        .remove("events_path");
    let err = parse_package(&manifest.to_string()).unwrap_err();
    assert!(messages(&err).contains("run.events_path: модуль подписан на события"));
    let mut manifest = base;
    manifest["run"].as_object_mut().unwrap().remove("cpus");
    manifest["run"].as_object_mut().unwrap().remove("memory_mb");
    manifest["run"].as_object_mut().unwrap().remove("internet");
    let parsed = parse_package(&manifest.to_string()).unwrap();
    let run = parsed.run.unwrap();
    assert_eq!(run.cpu_tenths, 10);
    assert_eq!(run.memory_mb, 256);
    assert!(!run.internet);
}

#[test]
fn раздел_карточки_кнопка_бота_и_входящий_путь_разбираются() {
    let image = fake_image();
    let mut manifest: Value = serde_json::from_str(&module_json(&[(
        "linux/amd64",
        "image-linux-amd64.tar",
        &image,
    )]))
    .unwrap();
    manifest["permissions"] = serde_json::json!(["clients_read", "clients_write"]);
    manifest["client_card"] = serde_json::json!({ "path": "/card", "title": "Бонус-игра" });
    manifest["bot"] = serde_json::json!({ "button": "🎮 Игра", "path": "/bot/screen" });
    manifest["run"]["inbound_path"] = serde_json::json!("/in");
    let parsed = parse_package(&manifest.to_string()).unwrap();
    let card = parsed.client_card.clone().unwrap();
    assert_eq!(card.path, "/card");
    assert_eq!(card.title, "Бонус-игра");
    assert_eq!(card.permission, sn_domain::access::Permission::ClientsRead);
    let bot = parsed.bot.clone().unwrap();
    assert_eq!(bot.button, "🎮 Игра");
    assert_eq!(bot.path, "/bot/screen");
    assert_eq!(
        parsed.run.as_ref().unwrap().inbound_path.as_deref(),
        Some("/in")
    );
    let stored = serde_json::to_value(&parsed).unwrap();
    let back: crate::ext_modules::manifest::Manifest = serde_json::from_value(stored).unwrap();
    assert_eq!(back, parsed);
    let mut old = serde_json::to_value(&parsed).unwrap();
    old.as_object_mut().unwrap().remove("client_card");
    old.as_object_mut().unwrap().remove("bot");
    old["run"].as_object_mut().unwrap().remove("inbound_path");
    let old: crate::ext_modules::manifest::Manifest = serde_json::from_value(old).unwrap();
    assert!(old.client_card.is_none() && old.bot.is_none());
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "client_card",
            serde_json::json!({ "path": "card" }),
            "client_card.path: путь внутри модуля начинается с /",
        ),
        (
            "client_card",
            serde_json::json!({ "path": "/card", "permission": "admins" }),
            "client_card.permission: право «Администраторы» модулю дать нельзя",
        ),
        (
            "client_card",
            serde_json::json!({ "path": "/card", "extra": 1 }),
            "client_card.extra: неизвестное поле",
        ),
        (
            "bot",
            serde_json::json!({ "path": "/bot" }),
            "bot.button: поле обязательно",
        ),
        (
            "bot",
            serde_json::json!({ "button": "Очень длинная надпись на кнопке модуля в боте", "path": "/bot" }),
            "bot.button: не длиннее 32 символов",
        ),
        (
            "bot",
            serde_json::json!({ "button": "Игра", "path": "/../bot" }),
            "bot.path: в пути не бывает",
        ),
        ("bot", serde_json::json!("Игра"), "bot: ожидается объект"),
    ];
    for (field, value, expected) in cases {
        let mut broken = manifest.clone();
        broken[field] = value;
        let err = parse_package(&broken.to_string()).unwrap_err();
        let text = messages(&err);
        assert!(text.contains(expected), "{field}: {text}");
    }
    let mut no_read = manifest.clone();
    no_read["permissions"] = serde_json::json!(["marketing"]);
    let err = parse_package(&no_read.to_string()).unwrap_err();
    assert!(
        messages(&err).contains("bot: экран в боте получает номер клиента"),
        "{}",
        messages(&err)
    );
    let mut inbound = manifest.clone();
    inbound["run"]["inbound_path"] = serde_json::json!("in");
    let err = parse_package(&inbound.to_string()).unwrap_err();
    assert!(messages(&err).contains("run.inbound_path: путь внутри модуля начинается с /"));
    let mut external = manifest;
    external.as_object_mut().unwrap().remove("run");
    external.as_object_mut().unwrap().remove("page");
    let err = parse(&external.to_string()).unwrap_err();
    let text = messages(&err);
    assert!(
        text.contains("client_card: Раздел в карточке клиента бывает только у модуля из архива"),
        "{text}"
    );
    assert!(
        text.contains("bot: Кнопка в боте бывает только у модуля из архива"),
        "{text}"
    );
}
