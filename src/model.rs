use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

pub const YOUTUBE: &str = "https://www.youtube.com/";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub adblock: bool,
    pub sponsorblock: bool,
    pub categories: Vec<String>,
    pub download_dir: PathBuf,
}

impl Settings {
    pub fn new(download_dir: PathBuf) -> Self {
        Self {
            adblock: true,
            sponsorblock: true,
            categories: vec!["sponsor".into(), "selfpromo".into()],
            download_dir,
        }
    }

    pub fn load_json(&mut self, bytes: &[u8]) -> serde_json::Result<()> {
        let data: Value = serde_json::from_slice(bytes)?;
        if let Some(value) = data["adblock"].as_bool() {
            self.adblock = value;
        }
        if let Some(value) = data["sponsorblock"].as_bool() {
            self.sponsorblock = value;
        }
        if let Some(values) = data["categories"].as_array() {
            self.categories = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        if let Some(path) = data["downloadDir"].as_str().filter(|path| !path.is_empty()) {
            self.download_dir = PathBuf::from(path);
        }
        Ok(())
    }

    pub fn page_settings(&self) -> Value {
        serde_json::json!({
            "adblock": self.adblock,
            "sponsorblock": self.sponsorblock,
            "categories": self.categories,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Setting {
    Adblock,
    Sponsorblock,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadMode {
    #[default]
    Mp4,
    Mp3,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DownloadState {
    pub active: bool,
    pub pct: i32,
    pub title: String,
    pub error: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PageMessage {
    Ready,
    Retry,
    SetSetting {
        key: Setting,
        value: bool,
    },
    Download {
        #[serde(default)]
        mode: DownloadMode,
    },
    DownloadState,
}

pub fn is_youtube(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
        )
}

pub fn video_url(url: &Url) -> Option<Url> {
    if !is_youtube(url) {
        return None;
    }
    let short = url.host_str() == Some("youtu.be");
    let id = if short {
        url.path().strip_prefix('/')?.to_owned()
    } else if url.path() == "/watch" {
        url.query_pairs()
            .find(|(key, _)| key == "v")?
            .1
            .into_owned()
    } else {
        return None;
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return None;
    }
    let mut canonical = Url::parse(YOUTUBE).expect("valid YouTube URL");
    canonical.set_path("/watch");
    canonical.query_pairs_mut().append_pair("v", &id);
    Some(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_python_settings_and_keeps_the_download_path_out_of_page_messages() {
        let mut settings = Settings::new("Videos/MaklyxaTube".into());
        settings
            .load_json(
                br#"{
            "adblock": false, "sponsorblock": true,
            "categories": ["sponsor", null, "selfpromo"],
            "downloadDir": "D:\\clips"
        }"#,
            )
            .unwrap();
        assert!(!settings.adblock);
        assert!(settings.sponsorblock);
        assert_eq!(settings.categories, ["sponsor", "selfpromo"]);
        assert_eq!(settings.download_dir, PathBuf::from(r"D:\clips"));
        assert_eq!(settings.page_settings()["adblock"], false);
        assert!(settings.page_settings().get("downloadDir").is_none());
        let saved = serde_json::to_vec(&settings).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&saved).unwrap()["downloadDir"],
            r"D:\clips"
        );
    }

    #[test]
    fn canonicalizes_watch_and_share_links() {
        for input in [
            "https://www.youtube.com/watch?list=PL1&v=abcDEF123_-&t=20",
            "https://m.youtube.com/watch?v=abcDEF123_-",
            "https://youtu.be/abcDEF123_-?si=test",
        ] {
            assert_eq!(
                video_url(&Url::parse(input).unwrap()).unwrap().as_str(),
                "https://www.youtube.com/watch?v=abcDEF123_-"
            );
        }
    }

    #[test]
    fn rejects_other_origins_and_invalid_video_ids() {
        for input in [
            "https://youtube.com.example.com/watch?v=abcDEF123_-",
            "https://example.com/?q=youtube.com/watch?v=abcDEF123_-",
            "https://youtube.com@example.com/watch?v=abcDEF123_-",
            "https://example.com@youtube.com/watch?v=abcDEF123_-",
            "https://youtube.com:456/watch?v=abcDEF123_-",
            "http://youtube.com/watch?v=abcDEF123_-",
            "https://youtube.com/watch?v=abcDEF123_--",
            "https://youtu.be/abcDEF123_-/more",
        ] {
            assert!(video_url(&Url::parse(input).unwrap()).is_none(), "{input}");
        }
    }

    #[test]
    fn accepts_the_bridge_commands() {
        assert!(matches!(
            serde_json::from_str::<PageMessage>(r#"{"type":"ready"}"#).unwrap(),
            PageMessage::Ready
        ));
        assert!(matches!(
            serde_json::from_str::<PageMessage>(r#"{"type":"retry"}"#).unwrap(),
            PageMessage::Retry
        ));
        assert!(matches!(
            serde_json::from_str::<PageMessage>(
                r#"{"type":"set_setting","key":"adblock","value":false}"#
            )
            .unwrap(),
            PageMessage::SetSetting {
                key: Setting::Adblock,
                value: false
            }
        ));
        assert!(matches!(
            serde_json::from_str::<PageMessage>(r#"{"type":"download","mode":"mp3"}"#).unwrap(),
            PageMessage::Download {
                mode: DownloadMode::Mp3
            }
        ));
        assert!(matches!(
            serde_json::from_str::<PageMessage>(r#"{"type":"download_state"}"#).unwrap(),
            PageMessage::DownloadState
        ));
        assert_eq!(DownloadState::default().pct, 0);
    }
}
