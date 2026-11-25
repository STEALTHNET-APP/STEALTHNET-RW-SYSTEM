use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::landing::ImageRulesView;
use crate::own_css::{
    CssCheck, CssPlaces, CssReport, ImageCodes, OwnCss, SiteImages, StyleFile, UploadedImage,
};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sn_domain::cabinet_css::{self, CabinetCss, CssPlace, STORED_IMAGES_MAX};
use sn_domain::css::{BLOCK_CSS_MAX, PAGE_CSS_MAX};
use sn_domain::ids::AdminId;
use sn_domain::image::ImageKind;
use sn_domain::settings::keys;
use sn_store::Db;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const ASSET_PREFIX: &str = "cabinet.";
pub const INVALID: &str = "cabinet.invalid";

pub const IMAGES: SiteImages = SiteImages {
    prefix: ASSET_PREFIX,
    codes: ImageCodes {
        empty: "cabinet_image.empty",
        too_heavy: "cabinet_image.too_heavy",
        format: "cabinet_image.format",
        damaged: "cabinet_image.damaged",
        too_small: "cabinet_image.too_small",
        too_large: "cabinet_image.too_large",
        limit: "cabinet_image.limit",
    },
    limit: STORED_IMAGES_MAX,
    full: "Для кабинета уже загружено 40 картинок — это предел. Картинки, которых нет в стилях, удаляются через час, после этого загрузка снова заработает",
    path: cabinet_css::image_path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CssLimits {
    pub page_css: usize,
    pub block_css: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CabinetView {
    pub css: CabinetCss,
    pub styles: BTreeMap<CssPlace, String>,
    pub limits: CssLimits,
    pub image: ImageRulesView,
    pub english: bool,
    pub off: Option<&'static str>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CabinetUpdate {
    #[serde(default)]
    pub css: Option<CssPlaces>,
}

pub struct CabinetOps<'a> {
    services: &'a Services,
}

impl<'a> CabinetOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        CabinetOps { services }
    }

    fn own_css(&self) -> OwnCss<'a> {
        OwnCss::new(self.services, ASSET_PREFIX, INVALID)
    }

    pub fn css(&self) -> CabinetCss {
        self.services.settings().cabinet_css.clone()
    }

    pub fn style(&self) -> Option<Arc<StyleFile>> {
        self.services.cabinet_style()
    }

    pub fn view(&self) -> CabinetView {
        let settings = self.services.settings();
        CabinetView {
            css: settings.cabinet_css.clone(),
            styles: settings.cabinet_css.styles(),
            limits: CssLimits {
                page_css: PAGE_CSS_MAX,
                block_css: BLOCK_CSS_MAX,
            },
            image: ImageRulesView::landing(),
            english: settings.languages.several(),
            off: (!settings.client_access.site_login)
                .then_some("Вход на сайт выключен в «Кабинет и мини-приложение»"),
        }
    }

    pub async fn check_css(
        &self,
        check: &CssCheck,
    ) -> ServiceResult<BTreeMap<CssPlace, CssReport>> {
        self.own_css().check(check).await
    }

    fn referenced(css: &CabinetCss) -> Vec<String> {
        css.images()
            .into_iter()
            .map(|(_, _, tag)| IMAGES.asset_name(&tag))
            .collect()
    }

    pub async fn update(&self, raw: &Value, admin: AdminId) -> ServiceResult<CabinetView> {
        let update = CabinetUpdate::deserialize(raw)
            .map_err(|_| ServiceError::rejected(INVALID, "CSS заполнен неверно"))?;
        let Some(places) = update.css else {
            return Ok(self.view());
        };
        let next = self.own_css().parse::<CssPlace>(&places).await?;
        let current = self.css();
        if next == current {
            return Ok(self.view());
        }
        let keep = Self::referenced(&next);
        let mut tx = self.services.store().begin().await?;
        if next.is_empty() {
            tx.settings().delete(keys::CABINET_CSS).await?;
        } else {
            tx.settings()
                .set_as(keys::CABINET_CSS, &next, Some(admin))
                .await?;
        }
        let removed = IMAGES.prune(&mut tx, &keep).await?;
        let details = json!({
            "css": current.changed(&next),
            "css_size": next.size(),
            "removed_images": removed,
        });
        tx.audit()
            .record(&Actor::Admin(admin).audit("settings.cabinet", "settings", None, details))
            .await?;
        tx.commit().await?;
        self.services.reload().await?;
        self.services.notify_settings_changed().await?;
        Ok(self.view())
    }

    pub async fn upload_image(&self, bytes: &[u8], admin: AdminId) -> ServiceResult<UploadedImage> {
        let keep = Self::referenced(&self.css());
        let mut tx = self.services.store().begin().await?;
        let (image, details) = IMAGES.store(&mut tx, bytes, admin, &keep).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit("settings.cabinet_image", "settings", None, details))
            .await?;
        tx.commit().await?;
        Ok(image)
    }

    pub async fn image_file(&self, tag: &str) -> ServiceResult<Option<(ImageKind, Vec<u8>)>> {
        IMAGES.file(self.services, tag).await
    }
}
