//! Browser state shared by the native surface, workbench chrome and persistence.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserTab {
    pub id: String,
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub start: bool,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub loading: bool,
    #[serde(default)]
    pub can_back: bool,
    #[serde(default)]
    pub can_forward: bool,
    #[serde(default = "zoom_default")]
    pub zoom: f64,
    #[serde(default)]
    pub error: Option<String>,
}
fn zoom_default() -> f64 {
    1.0
}
pub fn is_library_page(url: &str) -> bool {
    matches!(url, "about:blank#vivian-bookmarks" | "about:blank#vivian-downloads" | "about:blank#vivian-history" | "about:blank#vivian-extensions")
}
impl BrowserTab {
    pub fn new(url: String, private: bool) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: url.clone(),
            start: url == "about:blank" || is_library_page(&url),
            url,
            private,
            loading: true,
            can_back: false,
            can_forward: false,
            zoom: 1.0,
            error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageEntry {
    pub url: String,
    pub title: String,
    pub at: i64,
}
#[derive(Clone, Debug, Serialize)]
pub struct BrowserPermission {
    pub id: String,
    pub tab_id: String,
    pub origin: String,
    pub kind: i32,
}
#[derive(Clone, Debug, Serialize)]
pub struct BrowserExtension {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct BrowserDownload {
    pub id: String,
    pub path: String,
    pub received: i64,
    pub total: i64,
    pub state: i32,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BrowserSaved {
    #[serde(default)]
    pub tabs: Vec<BrowserTab>,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub history: Vec<PageEntry>,
    #[serde(default)]
    pub bookmarks: Vec<PageEntry>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct BrowserSnapshot {
    pub background_tasks: usize,
    pub revision: u64,
    pub tabs: Vec<BrowserTab>,
    pub active: Option<String>,
    pub history: Vec<PageEntry>,
    pub bookmarks: Vec<PageEntry>,
    pub permissions: Vec<BrowserPermission>,
    pub downloads: Vec<BrowserDownload>,
    pub extensions: Vec<BrowserExtension>,
    pub error: Option<String>,
}
impl BrowserSnapshot {
    /// Private sessions never enter the restart record.
    pub fn saved(&self) -> BrowserSaved {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| !tab.private)
            .cloned()
            .collect();
        let active = self
            .active
            .as_ref()
            .filter(|id| tabs.iter().any(|tab| &tab.id == *id))
            .cloned()
            .or_else(|| tabs.first().map(|tab| tab.id.clone()));
        BrowserSaved {
            tabs,
            active,
            history: self.history.clone(),
            bookmarks: self.bookmarks.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl BrowserBounds {
    pub fn validate(&self) -> Result<(), String> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .any(|n| !n.is_finite())
            || self.width < 0.0
            || self.height < 0.0
        {
            return Err("Invalid browser viewport".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserRequest {
    Init,
    NewTab {
        input: String,
        #[serde(default)]
        private: bool,
    },
    Navigate {
        id: String,
        input: String,
    },
    Select {
        id: String,
    },
    Close {
        id: String,
    },
    Reopen,
    Action {
        id: String,
        name: String,
        #[serde(default)]
        text: String,
    },
    Bookmark {
        id: String,
    },
    RemoveBookmark {
        url: String,
    },
    RenameBookmark {
        url: String,
        title: String,
    },
    Permission {
        id: String,
        allow: bool,
    },
    Download {
        id: String,
        name: String,
    },
    Extension {
        id: String,
        name: String,
        #[serde(default)]
        path: String,
    },
    ClearHistory,
}

pub fn address(input: &str) -> Result<String, String> {
    let text = input.trim();
    if text.is_empty() {
        return Ok("about:blank".into());
    }
    if std::path::Path::new(text).is_absolute() {
        return tauri::Url::from_file_path(text)
            .map(|u| u.to_string())
            .map_err(|_| "Invalid file path".into());
    }
    let explicit = text.contains("://") || text.starts_with("about:") || text.starts_with("data:");
    let candidate = if explicit {
        text.to_string()
    } else if !text.chars().any(char::is_whitespace)
        && (text.contains('.') || text.starts_with("localhost") || text.starts_with("[::1]"))
    {
        let local = text == "localhost"
            || text.starts_with("localhost:")
            || text.starts_with("localhost/")
            || text.starts_with("127.")
            || text.starts_with("[::1]");
        format!("{}://{text}", if local { "http" } else { "https" })
    } else {
        format!(
            "https://www.bing.com/search?q={}",
            urlencoding::encode(text)
        )
    };
    let url = tauri::Url::parse(&candidate).map_err(|e| e.to_string())?;
    match url.scheme() {
        "http" | "https" | "file" | "data" | "edge" | "about" | "chrome-extension" => {
            Ok(url.to_string())
        }
        _ => Err("Unsupported address scheme".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn addresses_and_private_restoration() {
        assert_eq!(address("example.com/a").unwrap(), "https://example.com/a");
        assert_eq!(
            address("http://localhost:3000").unwrap(),
            "http://localhost:3000/"
        );
        assert_eq!(address("localhost:3000").unwrap(), "http://localhost:3000/");
        assert!(address("two words").unwrap().contains("two%20words"));
        assert_eq!(address("").unwrap(), "about:blank");
        assert!(address("tauri://localhost").is_err());
        let normal = BrowserTab::new("https://example.com".into(), false);
        let private = BrowserTab::new("https://private.test".into(), true);
        let snapshot = BrowserSnapshot {
            active: Some(private.id.clone()),
            tabs: vec![normal.clone(), private],
            ..Default::default()
        };
        assert_eq!(snapshot.saved().tabs.len(), 1);
        assert_eq!(snapshot.saved().active, Some(normal.id));
        assert!(BrowserBounds {
            width: f64::NAN,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
