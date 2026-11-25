use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sn_domain::css::{self, CssError, Place, PlacedCss};
use sn_domain::ids::AdminId;
use sn_domain::image::{self, ImageKind, ImageProblem, LANDING_RULES};
use sn_domain::landing::{IMAGE_TAG_LEN, valid_tag};
use sn_store::{Db, SiteAsset, Tx};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const VERSION_LEN: usize = 16;

pub type CssPlaces = BTreeMap<String, Option<String>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleFile {
    pub version: String,
    pub text: String,
    files: &'static str,
}

impl StyleFile {
    pub fn of<P: Place>(css: &PlacedCss<P>) -> Option<Arc<StyleFile>> {
        let text = css.style();
        (!text.is_empty()).then(|| {
            Arc::new(StyleFile {
                version: short_hash(text.as_bytes()),
                text,
                files: P::SITE.files,
            })
        })
    }

    pub fn path(&self) -> String {
        format!("{}{}.css", self.files, self.version)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CssReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl From<CssError> for CssReport {
    fn from(err: CssError) -> CssReport {
        CssReport {
            style: None,
            line: err.line,
            problem: Some(err.reason),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CssCheck {
    #[serde(default)]
    pub css: CssPlaces,
}

pub fn short_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))[..VERSION_LEN].to_string()
}

fn lost_image() -> String {
    "картинка не найдена, загрузите её заново".to_string()
}

pub struct OwnCss<'a> {
    services: &'a Services,
    asset_prefix: &'static str,
    invalid: &'static str,
}

impl<'a> OwnCss<'a> {
    pub fn new(
        services: &'a Services,
        asset_prefix: &'static str,
        invalid: &'static str,
    ) -> OwnCss<'a> {
        OwnCss {
            services,
            asset_prefix,
            invalid,
        }
    }

    async fn missing(&self, tags: BTreeSet<String>) -> ServiceResult<BTreeSet<String>> {
        if tags.is_empty() {
            return Ok(tags);
        }
        let asset_name = |tag: &String| format!("{}{tag}", self.asset_prefix);
        let names: Vec<String> = tags.iter().map(asset_name).collect();
        let mut conn = self.services.store().conn().await?;
        let found: BTreeSet<String> = conn
            .site_assets()
            .existing(&names)
            .await?
            .into_iter()
            .collect();
        Ok(tags
            .into_iter()
            .filter(|tag| !found.contains(&asset_name(tag)))
            .collect())
    }

    pub async fn check<P: Place>(&self, check: &CssCheck) -> ServiceResult<BTreeMap<P, CssReport>> {
        let mut checked = BTreeMap::new();
        for (name, text) in &check.css {
            if let Some(place) = P::parse(name) {
                let verdict = css::check(place, text.as_deref().unwrap_or_default());
                checked.insert(place, verdict);
            }
        }
        let tags = checked
            .values()
            .flatten()
            .flat_map(|css| css.images.keys().cloned())
            .collect();
        let missing = self.missing(tags).await?;
        Ok(checked
            .into_iter()
            .map(|(place, verdict)| {
                let report = match verdict {
                    Err(err) => CssReport::from(err),
                    Ok(css) => match css.images.iter().find(|(tag, _)| missing.contains(*tag)) {
                        Some((_, line)) => CssReport {
                            style: None,
                            line: Some(*line),
                            problem: Some(lost_image()),
                        },
                        None => CssReport {
                            style: Some(css.style),
                            ..CssReport::default()
                        },
                    },
                };
                (place, report)
            })
            .collect())
    }

    pub async fn parse<P: Place>(&self, places: &CssPlaces) -> ServiceResult<PlacedCss<P>> {
        let css = PlacedCss::<P>::parse(places)
            .map_err(|err| ServiceError::rejected(self.invalid, err.to_string()))?;
        let images = css.images();
        let missing = self
            .missing(images.iter().map(|(_, _, tag)| tag.clone()).collect())
            .await?;
        if let Some((place, line, _)) = images.iter().find(|(_, _, tag)| missing.contains(tag)) {
            return Err(ServiceError::rejected(
                self.invalid,
                format!("{}, строка {line}: {}", place.title(), lost_image()),
            ));
        }
        Ok(css)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UploadedImage {
    pub tag: String,
    pub kind: ImageKind,
    pub width: u32,
    pub height: u32,
    pub size: u32,
    pub url: String,
}

pub struct ImageCodes {
    pub empty: &'static str,
    pub too_heavy: &'static str,
    pub format: &'static str,
    pub damaged: &'static str,
    pub too_small: &'static str,
    pub too_large: &'static str,
    pub limit: &'static str,
}

pub struct SiteImages {
    pub prefix: &'static str,
    pub codes: ImageCodes,
    pub limit: usize,
    pub full: &'static str,
    pub path: fn(&str, ImageKind) -> String,
}

pub const STALE_AFTER_SECONDS: i64 = 3600;

impl SiteImages {
    pub fn asset_name(&self, tag: &str) -> String {
        format!("{}{tag}", self.prefix)
    }

    pub fn refusal(&self, problem: ImageProblem) -> ServiceError {
        let codes = &self.codes;
        let (code, message) = match problem {
            ImageProblem::Empty => (codes.empty, "Файл пустой".to_string()),
            ImageProblem::TooHeavy => (
                codes.too_heavy,
                "Файл больше 1 МБ: уменьшите картинку".to_string(),
            ),
            ImageProblem::Format | ImageProblem::NotSquare => {
                (codes.format, "Подходят только PNG, JPEG и WebP".to_string())
            }
            ImageProblem::Damaged => (
                codes.damaged,
                "Картинка повреждена или сохранена не до конца".to_string(),
            ),
            ImageProblem::TooSmall => (
                codes.too_small,
                format!(
                    "Картинка слишком маленькая: нужно от {} точек по каждой стороне",
                    LANDING_RULES.min_side
                ),
            ),
            ImageProblem::TooLarge => (
                codes.too_large,
                format!(
                    "Картинка больше {} точек по стороне",
                    LANDING_RULES.max_side
                ),
            ),
        };
        ServiceError::rejected(code, message)
    }

    pub async fn prune(&self, tx: &mut Tx, keep: &[String]) -> ServiceResult<u64> {
        Ok(tx
            .site_assets()
            .prune(self.prefix, keep, STALE_AFTER_SECONDS)
            .await?)
    }

    pub async fn store(
        &self,
        tx: &mut Tx,
        bytes: &[u8],
        admin: AdminId,
        keep: &[String],
    ) -> ServiceResult<(UploadedImage, Value)> {
        let info = image::check(bytes, &LANDING_RULES).map_err(|problem| self.refusal(problem))?;
        let digest = hex::encode(Sha256::digest(bytes));
        let tag = digest[..IMAGE_TAG_LEN].to_string();
        let name = self.asset_name(&tag);
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let limit = i64::try_from(self.limit).unwrap_or(i64::MAX);
        if !tx.site_assets().exists(&name).await?
            && tx.site_assets().count_prefix(self.prefix).await? >= limit
        {
            self.prune(tx, keep).await?;
            if tx.site_assets().count_prefix(self.prefix).await? >= limit {
                return Err(ServiceError::rejected(self.codes.limit, self.full));
            }
        }
        tx.site_assets()
            .put(
                &name,
                &SiteAsset {
                    content_type: info.kind.content_type().to_string(),
                    bytes: bytes.to_vec(),
                    sha256: digest.clone(),
                },
                Some(admin),
            )
            .await?;
        let details = json!({
            "action": "uploaded",
            "kind": info.kind,
            "width": info.width,
            "height": info.height,
            "size": size,
            "sha256": digest,
        });
        let image = UploadedImage {
            url: (self.path)(&tag, info.kind),
            tag,
            kind: info.kind,
            width: info.width,
            height: info.height,
            size,
        };
        Ok((image, details))
    }

    pub async fn file(
        &self,
        services: &Services,
        tag: &str,
    ) -> ServiceResult<Option<(ImageKind, Vec<u8>)>> {
        if !valid_tag(tag) {
            return Ok(None);
        }
        let mut conn = services.store().conn().await?;
        let Some(asset) = conn.site_assets().get(&self.asset_name(tag)).await? else {
            return Ok(None);
        };
        if !asset.sha256.starts_with(tag) {
            return Ok(None);
        }
        let kind = ImageKind::sniff(&asset.bytes).filter(|kind| LANDING_RULES.kinds.contains(kind));
        Ok(kind.map(|kind| (kind, asset.bytes)))
    }
}
