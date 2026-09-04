use std::{fmt, str::FromStr, time::Duration};

use serde::{Deserialize, Serialize};

use crate::XReadError;

pub const DEFAULT_COMMUNITY_BASE_URL: &str = "https://api.fxtwitter.com/2";
pub const DEFAULT_LIMIT: usize = 20;
pub const MAX_LIMIT: usize = 1_000;
pub const DEFAULT_TIMEOUT_MS: u64 = 12_000;
pub const DEFAULT_RETRIES: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    Direct,
    Thread,
}

impl FromStr for ReplyMode {
    type Err = XReadError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "direct" | "replies" => Ok(Self::Direct),
            "thread" => Ok(Self::Thread),
            _ => Err(XReadError::invalid_input(
                "replies 只接受 direct、replies 或 thread。",
            )),
        }
    }
}

impl fmt::Display for ReplyMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Direct => "direct",
            Self::Thread => "thread",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortMode {
    #[default]
    Relevance,
    Recent,
}

impl SortMode {
    pub(crate) fn community_value(self) -> &'static str {
        match self {
            Self::Relevance => "likes",
            Self::Recent => "recency",
        }
    }
}

impl FromStr for SortMode {
    type Err = XReadError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "relevance" => Ok(Self::Relevance),
            "recent" => Ok(Self::Recent),
            _ => Err(XReadError::invalid_input(
                "sort 只接受 relevance 或 recent。",
            )),
        }
    }
}

impl fmt::Display for SortMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Relevance => "relevance",
            Self::Recent => "recent",
        })
    }
}

/// Which MP4 representation to select for each video.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VideoQuality {
    #[default]
    Best,
    Worst,
    /// For portrait video, 720x1280 is treated as 720p (the shorter edge).
    Height(u32),
}

impl FromStr for VideoQuality {
    type Err = XReadError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "best" => Ok(Self::Best),
            "worst" => Ok(Self::Worst),
            _ => {
                let height = normalized
                    .strip_suffix('p')
                    .unwrap_or(&normalized)
                    .parse::<u32>()
                    .ok()
                    .filter(|height| (144..=4_320).contains(height))
                    .ok_or_else(|| {
                        XReadError::invalid_input(
                            "quality 只接受 best、worst 或 144 到 4320 的清晰度，例如 720。",
                        )
                    })?;
                Ok(Self::Height(height))
            }
        }
    }
}

impl fmt::Display for VideoQuality {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Best => formatter.write_str("best"),
            Self::Worst => formatter.write_str("worst"),
            Self::Height(height) => write!(formatter, "{height}p"),
        }
    }
}

/// Options that affect the meaning of a read operation.
#[derive(Debug, Clone)]
pub struct ReadOptions {
    pub reply_mode: Option<ReplyMode>,
    pub limit: usize,
    pub sort: SortMode,
    pub lang: String,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            reply_mode: None,
            limit: DEFAULT_LIMIT,
            sort: SortMode::Relevance,
            lang: "en".to_owned(),
        }
    }
}

impl ReadOptions {
    pub fn validate(&self) -> Result<(), XReadError> {
        if !(1..=MAX_LIMIT).contains(&self.limit) {
            return Err(XReadError::invalid_input(format!(
                "limit 必须在 1 到 {MAX_LIMIT} 之间。"
            )));
        }
        if !valid_language(&self.lang) {
            return Err(XReadError::invalid_input(
                "lang 需要语言代码，例如 en、zh-cn 或 ja。",
            ));
        }
        Ok(())
    }
}

/// Transport settings are separate from `ReadOptions`, so a Worker can own
/// deployment policy (upstream host, timeout, retries) while callers choose
/// only what content they want.
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub community_base_url: String,
    pub timeout: Duration,
    pub retries: u8,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            community_base_url: DEFAULT_COMMUNITY_BASE_URL.to_owned(),
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            retries: DEFAULT_RETRIES,
        }
    }
}

impl ReaderConfig {
    pub fn validate(&self) -> Result<(), XReadError> {
        let milliseconds = self.timeout.as_millis();
        if !(1_000..=120_000).contains(&milliseconds) {
            return Err(XReadError::invalid_input(
                "timeout 必须在 1000 到 120000 毫秒之间。",
            ));
        }
        if self.retries > 5 {
            return Err(XReadError::invalid_input("retries 必须在 0 到 5 之间。"));
        }
        Ok(())
    }
}

fn valid_language(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    if !(2..=3).contains(&first.len()) || !first.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return false;
    }
    parts.all(|part| {
        (2..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostReference {
    pub id: String,
    pub original: String,
    pub username: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub post: Post,
    pub replies: Vec<Post>,
    pub reply_info: Option<ReplyInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplyInfo {
    pub available_count: Option<u64>,
    pub backend: String,
    pub conversation_id: Option<String>,
    pub count: usize,
    pub error: Option<String>,
    pub limit: usize,
    pub mode: String,
    pub range: String,
    pub sort: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    pub article: Option<Article>,
    pub author: Author,
    pub conversation_id: Option<String>,
    pub created_at: Option<String>,
    pub date_label: Option<String>,
    pub id: Option<String>,
    pub is_long_post: bool,
    pub kind: String,
    pub lang: Option<String>,
    pub links: Vec<Link>,
    pub media: Vec<Media>,
    pub metrics: Option<Metrics>,
    pub parent_id: Option<String>,
    pub possibly_sensitive: Option<bool>,
    pub quoted_post: Option<Box<Post>>,
    pub reposted_by: Option<Author>,
    pub reposted_post: Option<Box<Post>>,
    pub source: String,
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    pub id: Option<String>,
    pub name: Option<String>,
    pub profile_image_url: Option<String>,
    pub username: Option<String>,
    pub verified: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Article {
    pub cover_media_url: Option<String>,
    pub created_at: Option<String>,
    pub embedded_post_ids: Vec<String>,
    pub id: Option<String>,
    pub links: Vec<Link>,
    pub markdown: String,
    /// Media embedded inside the Article body, in upstream order.
    #[serde(default)]
    pub media: Vec<Media>,
    pub modified_at: Option<String>,
    pub preview_text: Option<String>,
    pub text: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Link {
    pub href: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub alt_text: Option<String>,
    pub duration_ms: Option<u64>,
    pub height: Option<u64>,
    pub preview_image_url: Option<String>,
    #[serde(rename = "type")]
    pub media_type: Option<String>,
    pub url: Option<String>,
    pub variants: Vec<MediaVariant>,
    pub width: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaVariant {
    pub bit_rate: Option<u64>,
    pub content_type: Option<String>,
    pub url: String,
}

impl Post {
    /// Return one directly downloadable URL for every video/GIF in this post.
    ///
    /// FxTwitter may expose several encodings of one video. Picking the MP4
    /// variant with the greatest bitrate preserves xvdl's old "one URL per
    /// video" contract instead of returning every resolution as a new video.
    pub fn video_urls(&self) -> Vec<String> {
        self.video_urls_with_quality(VideoQuality::Best)
    }

    pub fn video_urls_with_quality(&self, quality: VideoQuality) -> Vec<String> {
        let mut urls = Vec::new();
        for media in &self.media {
            let selected = media.video_url_with_quality(quality);
            if let Some(url) = selected {
                if !urls.iter().any(|existing| existing == url) {
                    urls.push(url.to_owned());
                }
            }
        }
        urls
    }
}

impl Media {
    /// Select one directly usable MP4 representation for this media item.
    pub fn video_url(&self) -> Option<&str> {
        self.video_url_with_quality(VideoQuality::Best)
    }

    pub fn video_url_with_quality(&self, quality: VideoQuality) -> Option<&str> {
        if !matches!(self.media_type.as_deref(), Some("video" | "gif")) {
            return None;
        }
        let variants: Vec<_> = self
            .variants
            .iter()
            .filter(|variant| is_mp4_variant(variant))
            .collect();
        select_video_variant(&variants, quality)
            .map(|variant| variant.url.as_str())
            // Some upstream media objects use `url` for a thumbnail. Only
            // accept the fallback when it is itself an MP4.
            .or_else(|| {
                self.url
                    .as_deref()
                    .filter(|url| url.to_ascii_lowercase().contains(".mp4"))
            })
    }
}

fn select_video_variant<'a>(
    variants: &[&'a MediaVariant],
    quality: VideoQuality,
) -> Option<&'a MediaVariant> {
    match quality {
        VideoQuality::Best => variants
            .iter()
            .copied()
            .max_by_key(|variant| variant.bit_rate.unwrap_or(0)),
        VideoQuality::Worst => variants
            .iter()
            .copied()
            .min_by_key(|variant| variant.bit_rate.unwrap_or(0)),
        VideoQuality::Height(target) => {
            let known: Vec<_> = variants
                .iter()
                .copied()
                .filter_map(|variant| {
                    variant_resolution(&variant.url).map(|height| (variant, height))
                })
                .collect();
            known
                .iter()
                .copied()
                .filter(|(_, height)| *height <= target)
                .max_by_key(|(variant, height)| (*height, variant.bit_rate.unwrap_or(0)))
                .or_else(|| known.iter().copied().min_by_key(|(_, height)| *height))
                .map(|(variant, _)| variant)
                .or_else(|| {
                    variants
                        .iter()
                        .copied()
                        .max_by_key(|variant| variant.bit_rate.unwrap_or(0))
                })
        }
    }
}

fn variant_resolution(url: &str) -> Option<u32> {
    url.split('/').find_map(|segment| {
        let (width, height) = segment.split_once('x')?;
        Some(width.parse::<u32>().ok()?.min(height.parse::<u32>().ok()?))
    })
}

fn is_mp4_variant(variant: &MediaVariant) -> bool {
    variant
        .content_type
        .as_deref()
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("mp4"))
        || variant.url.to_ascii_lowercase().contains(".mp4")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Metrics {
    pub bookmark_count: Option<u64>,
    pub impression_count: Option<u64>,
    pub like_count: Option<u64>,
    pub quote_count: Option<u64>,
    pub reply_count: Option<u64>,
    pub retweet_count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_options_at_public_boundary() {
        let mut options = ReadOptions {
            lang: "zh-cn".to_owned(),
            ..ReadOptions::default()
        };
        assert!(options.validate().is_ok());

        options.lang = "not_a_language".to_owned();
        assert!(options.validate().is_err());
    }

    #[test]
    fn selects_one_highest_bitrate_url_per_video() {
        let video = Media {
            alt_text: None,
            duration_ms: None,
            height: None,
            preview_image_url: None,
            media_type: Some("video".to_owned()),
            url: Some("https://video.example/fallback.mp4".to_owned()),
            variants: vec![
                MediaVariant {
                    bit_rate: None,
                    content_type: Some("m3u8".to_owned()),
                    url: "https://video.example/playlist.m3u8".to_owned(),
                },
                MediaVariant {
                    bit_rate: Some(632_000),
                    content_type: Some("mp4".to_owned()),
                    url: "https://video.example/480x852/video.mp4".to_owned(),
                },
                MediaVariant {
                    bit_rate: Some(2_176_000),
                    content_type: Some("video/mp4".to_owned()),
                    url: "https://video.example/720x1280/video.mp4".to_owned(),
                },
            ],
            width: None,
        };
        let photo = Media {
            media_type: Some("photo".to_owned()),
            url: Some("https://image.example/photo.jpg".to_owned()),
            ..video.clone()
        };
        let post = Post {
            article: None,
            author: Author::default(),
            conversation_id: None,
            created_at: None,
            date_label: None,
            id: Some("1".to_owned()),
            is_long_post: false,
            kind: "post".to_owned(),
            lang: None,
            links: Vec::new(),
            media: vec![video, photo],
            metrics: None,
            parent_id: None,
            possibly_sensitive: None,
            quoted_post: None,
            reposted_by: None,
            reposted_post: None,
            source: "test".to_owned(),
            text: String::new(),
            url: None,
        };

        assert_eq!(
            post.video_urls(),
            vec!["https://video.example/720x1280/video.mp4"]
        );
        assert_eq!(
            post.video_urls_with_quality(VideoQuality::Height(480)),
            vec!["https://video.example/480x852/video.mp4"]
        );
        assert_eq!(
            post.video_urls_with_quality(VideoQuality::Worst),
            vec!["https://video.example/480x852/video.mp4"]
        );
    }

    #[test]
    fn parses_video_quality() {
        assert_eq!("best".parse(), Ok(VideoQuality::Best));
        assert_eq!("720p".parse(), Ok(VideoQuality::Height(720)));
        assert!("potato".parse::<VideoQuality>().is_err());
    }

    #[test]
    fn never_returns_a_thumbnail_as_a_video_fallback() {
        let post = Post {
            article: None,
            author: Author::default(),
            conversation_id: None,
            created_at: None,
            date_label: None,
            id: Some("1".to_owned()),
            is_long_post: false,
            kind: "post".to_owned(),
            lang: None,
            links: Vec::new(),
            media: vec![Media {
                alt_text: None,
                duration_ms: None,
                height: None,
                preview_image_url: None,
                media_type: Some("video".to_owned()),
                url: Some("https://image.example/thumbnail.jpg".to_owned()),
                variants: Vec::new(),
                width: None,
            }],
            metrics: None,
            parent_id: None,
            possibly_sensitive: None,
            quoted_post: None,
            reposted_by: None,
            reposted_post: None,
            source: "test".to_owned(),
            text: String::new(),
            url: None,
        };

        assert!(post.video_urls().is_empty());
    }
}
