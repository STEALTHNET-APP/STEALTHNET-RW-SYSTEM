use crate::telegram::Button;
use sn_services::bot_content::BotContent;

pub use sn_domain::bot_content::Value;
pub use sn_domain::bot_content::Value::{Html, Text};

pub type Vars<'a, 'b> = &'a [(&'a str, Value<'b>)];

pub fn text(content: &BotContent, key: &str, vars: Vars<'_, '_>) -> String {
    content.content.text(key, vars)
}

pub fn label(content: &BotContent, key: &str, vars: Vars<'_, '_>) -> String {
    content.content.label(key, vars)
}

pub fn button(
    content: &BotContent,
    key: &str,
    vars: Vars<'_, '_>,
    data: impl Into<String>,
) -> Option<Button> {
    let label = content.content.button(key, vars)?;
    Some(Button::callback(label.text, data).look(label.emoji_id, label.style.map(|s| s.as_str())))
}

pub fn link(
    content: &BotContent,
    key: &str,
    vars: Vars<'_, '_>,
    url: impl Into<String>,
) -> Option<Button> {
    let label = content.content.button(key, vars)?;
    Some(Button::url(label.text, url).look(label.emoji_id, label.style.map(|s| s.as_str())))
}

pub fn copy(content: &BotContent, key: &str, value: &str) -> Option<Button> {
    if value.is_empty() || value.chars().count() > crate::telegram::COPY_TEXT_MAX {
        return None;
    }
    let label = content.content.button(key, &[])?;
    Some(Button::copy(label.text, value).look(label.emoji_id, label.style.map(|s| s.as_str())))
}

pub fn web_app(content: &BotContent, key: &str, url: impl Into<String>) -> Option<Button> {
    let label = content.content.button(key, &[])?;
    Some(Button::web_app(label.text, url).look(label.emoji_id, label.style.map(|s| s.as_str())))
}

pub fn menu(content: &BotContent) -> Option<Button> {
    button(content, "common.btn.menu", &[], "m")
}

pub fn back(content: &BotContent, data: impl Into<String>) -> Option<Button> {
    button(content, "common.btn.back", &[], data)
}

pub fn cancel(content: &BotContent, data: impl Into<String>) -> Option<Button> {
    button(content, "common.btn.cancel", &[], data)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lines(Vec<String>);

impl Lines {
    pub fn new() -> Self {
        Lines(Vec::new())
    }

    pub fn add(&mut self, line: String) {
        if !line.is_empty() {
            self.0.push(line);
        }
    }

    pub fn blank(&mut self) {
        self.0.push(String::new());
    }

    pub fn text(self) -> String {
        self.0.join("\n")
    }
}
