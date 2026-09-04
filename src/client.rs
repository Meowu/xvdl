use std::{collections::HashSet, time::Duration};

use reqwest::{header::RETRY_AFTER, Client, Url};
use serde_json::Value;

use crate::{
    normalize::{is_community_status, normalize_community_post, normalize_oembed},
    parse_post_reference, ReadOptions, ReadResult, ReaderConfig, ReplyInfo, ReplyMode,
    VideoQuality, XReadError,
};

const OEMBED_ENDPOINT: &str = "https://publish.x.com/oembed";

/// Reusable application service. Both the CLI and the Worker call this type.
#[derive(Debug, Clone)]
pub struct XReader {
    client: Client,
    community_base_url: Url,
    timeout: Duration,
    retries: u8,
}

impl XReader {
    pub fn new(mut config: ReaderConfig) -> Result<Self, XReadError> {
        config.validate()?;
        config.community_base_url = config.community_base_url.trim_end_matches('/').to_owned();
        let community_base_url = Url::parse(&config.community_base_url).map_err(|_| {
            XReadError::invalid_input(
                "community base 需要有效的 http(s) URL，例如 https://api.fxtwitter.com/2。",
            )
        })?;
        if !matches!(community_base_url.scheme(), "http" | "https") {
            return Err(XReadError::invalid_input(
                "community base 只接受 http(s) URL。",
            ));
        }
        let client = Client::builder()
            .user_agent(format!("xread/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| XReadError::internal(format!("无法创建 HTTP 客户端：{error}")))?;

        Ok(Self {
            client,
            community_base_url,
            timeout: config.timeout,
            retries: config.retries,
        })
    }

    /// Read one post, optionally together with direct replies or its thread.
    ///
    /// The fallback policy lives here instead of in the CLI/Worker adapters:
    /// FxTwitter provides structured data; X oEmbed is a smaller official,
    /// token-free fallback that can still return the main post.
    pub async fn read(&self, input: &str, options: &ReadOptions) -> Result<ReadResult, XReadError> {
        options.validate()?;
        let reference = parse_post_reference(input)?;

        if let Some(mode) = options.reply_mode {
            match self
                .fetch_community_conversation(&reference.id, options)
                .await
            {
                Ok(conversation) => {
                    return Ok(ReadResult {
                        post: conversation.post,
                        replies: conversation.replies,
                        reply_info: Some(ReplyInfo {
                            available_count: conversation.available_count,
                            backend: "community".to_owned(),
                            conversation_id: None,
                            count: conversation.count,
                            error: None,
                            limit: options.limit,
                            mode: mode.to_string(),
                            range: "community-first-page".to_owned(),
                            sort: options.sort.to_string(),
                            truncated: conversation.truncated,
                        }),
                        warnings: vec![
                            "免费回复由第三方 FxTwitter 提供，只包含首批精选/最新评论，不保证完整。"
                                .to_owned(),
                        ],
                    });
                }
                Err(community_error) => {
                    let post = self
                        .fetch_oembed(&reference.id, &reference.url, &options.lang)
                        .await
                        .map_err(|oembed_error| {
                            combined_source_error(&community_error, &oembed_error)
                        })?;
                    return Ok(ReadResult {
                        post,
                        replies: Vec::new(),
                        reply_info: Some(ReplyInfo {
                            available_count: None,
                            backend: "community".to_owned(),
                            conversation_id: None,
                            count: 0,
                            error: Some("FxTwitter 免费结构化源不可用".to_owned()),
                            limit: options.limit,
                            mode: mode.to_string(),
                            range: "community-first-page".to_owned(),
                            sort: options.sort.to_string(),
                            truncated: true,
                        }),
                        warnings: vec![format!(
                            "免费回复源暂时不可用，已通过 X oEmbed 返回主推文：{}",
                            community_error.message
                        )],
                    });
                }
            }
        }

        match self.fetch_community_post(&reference.id).await {
            Ok(post) => Ok(ReadResult {
                post,
                replies: Vec::new(),
                reply_info: None,
                warnings: Vec::new(),
            }),
            Err(community_error) => {
                let post = self
                    .fetch_oembed(&reference.id, &reference.url, &options.lang)
                    .await
                    .map_err(|oembed_error| {
                        combined_source_error(&community_error, &oembed_error)
                    })?;
                Ok(ReadResult {
                    post,
                    replies: Vec::new(),
                    reply_info: None,
                    warnings: vec![format!(
                        "免费结构化源暂时不可用，已退回 X oEmbed；长推文、Article、引用和媒体信息可能不完整：{}",
                        community_error.message
                    )],
                })
            }
        }
    }

    /// Preserve xvdl's original API: fetch one post and return one best MP4
    /// URL for each video. oEmbed is not used here because it has no media data.
    pub async fn video_urls(&self, input: &str) -> Result<Vec<String>, XReadError> {
        self.video_urls_with_quality(input, VideoQuality::Best)
            .await
    }

    pub async fn video_urls_with_quality(
        &self,
        input: &str,
        quality: VideoQuality,
    ) -> Result<Vec<String>, XReadError> {
        let post = self.media_post(input).await?;
        let urls = post.video_urls_with_quality(quality);
        if urls.is_empty() {
            return Err(XReadError::no_video("该推文中没有找到可下载的视频。"));
        }
        Ok(urls)
    }

    /// Media operations need the structured source: oEmbed deliberately omits
    /// downloadable video variants, so falling back would look like "no video"
    /// even when the real problem was an unavailable upstream service.
    pub(crate) async fn media_post(&self, input: &str) -> Result<crate::Post, XReadError> {
        let reference = parse_post_reference(input)?;
        self.fetch_community_post(&reference.id).await
    }

    async fn fetch_community_post(&self, id: &str) -> Result<crate::Post, XReadError> {
        let url = self.community_url(&format!("status/{id}"))?;
        let payload = self.request_json(url, "FxTwitter 免费结构化源").await?;
        let status = payload.get("status").ok_or_else(missing_community_post)?;
        if !is_community_status(status) {
            return Err(missing_community_post());
        }
        normalize_community_post(status, 0)
    }

    async fn fetch_community_conversation(
        &self,
        id: &str,
        options: &ReadOptions,
    ) -> Result<Conversation, XReadError> {
        let mut url = self.community_url(&format!("conversation/{id}"))?;
        url.query_pairs_mut()
            .append_pair("ranking_mode", options.sort.community_value());
        let payload = self.request_json(url, "FxTwitter 免费回复源").await?;
        let status = payload.get("status").ok_or_else(missing_community_post)?;
        if !is_community_status(status) {
            return Err(missing_community_post());
        }

        let post = normalize_community_post(status, 0)?;
        let mut candidates = Vec::new();
        if options.reply_mode == Some(ReplyMode::Thread) {
            candidates.extend(json_array(payload.get("thread")));
        }
        candidates.extend(json_array(payload.get("replies")));

        let mut seen = HashSet::new();
        let mut replies = Vec::new();
        for candidate in candidates {
            if !is_community_status(candidate) {
                continue;
            }
            let normalized = normalize_community_post(candidate, 0)?;
            let Some(reply_id) = normalized.id.as_deref() else {
                continue;
            };
            if reply_id == id || !seen.insert(reply_id.to_owned()) {
                continue;
            }
            if options.reply_mode == Some(ReplyMode::Direct)
                && normalized.parent_id.as_deref() != Some(id)
            {
                continue;
            }
            replies.push(normalized);
            if replies.len() >= options.limit {
                break;
            }
        }
        let available_count = status.get("replies").and_then(json_u64);
        let truncated = payload
            .get("cursor")
            .and_then(|cursor| cursor.get("bottom"))
            .is_some_and(json_truthy)
            || available_count.is_some_and(|available| available > replies.len() as u64);
        let count = replies.len();

        Ok(Conversation {
            post,
            replies,
            available_count,
            count,
            truncated,
        })
    }

    async fn fetch_oembed(
        &self,
        id: &str,
        canonical_url: &str,
        lang: &str,
    ) -> Result<crate::Post, XReadError> {
        let mut url = Url::parse(OEMBED_ENDPOINT)
            .map_err(|error| XReadError::internal(format!("无效的 oEmbed 地址：{error}")))?;
        url.query_pairs_mut()
            .append_pair("url", canonical_url)
            .append_pair("omit_script", "true")
            .append_pair("hide_thread", "true")
            .append_pair("dnt", "true")
            .append_pair("lang", lang);
        let payload = self.request_json(url, "X oEmbed").await?;
        normalize_oembed(&payload, id)
    }

    fn community_url(&self, path: &str) -> Result<Url, XReadError> {
        // `Url::join` treats a base without a trailing slash as a file. Adding
        // it here means both ".../2" and ".../2/" behave identically.
        let base = format!(
            "{}/",
            self.community_base_url.as_str().trim_end_matches('/')
        );
        Url::parse(&base)
            .and_then(|base| base.join(path))
            .map_err(|error| XReadError::internal(format!("无法构造上游地址：{error}")))
    }

    async fn request_json(&self, url: Url, label: &str) -> Result<Value, XReadError> {
        let mut last_error = None;

        for attempt in 0..=self.retries {
            let response = match self
                .client
                .get(url.clone())
                .header("accept", "application/json")
                .timeout(self.timeout)
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    let normalized = if error.is_timeout() {
                        XReadError::upstream(format!(
                            "{label}超时（{}ms）。",
                            self.timeout.as_millis()
                        ))
                        .with_hint("检查网络后重试，或增大超时时间。")
                    } else {
                        XReadError::upstream(format!("{label}网络错误：{error}"))
                            .with_hint("检查网络、代理或防火墙设置后重试。")
                    };
                    if attempt == self.retries {
                        return Err(normalized);
                    }
                    last_error = Some(normalized);
                    portable_sleep(retry_delay(None, attempt)).await;
                    continue;
                }
            };

            let status = response.status();
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = match response.text().await {
                Ok(body) => body,
                Err(error) => {
                    let normalized =
                        XReadError::upstream(format!("无法读取 {label} 响应：{error}"));
                    if attempt == self.retries {
                        return Err(normalized);
                    }
                    last_error = Some(normalized);
                    portable_sleep(retry_delay(retry_after.as_deref(), attempt)).await;
                    continue;
                }
            };

            if status.is_success() {
                return serde_json::from_str(&body).map_err(|error| {
                    XReadError::invalid_response(format!("{label} 返回的不是有效 JSON：{error}"))
                });
            }

            let error = upstream_http_error(label, status.as_u16(), &body);
            if !retryable_status(status.as_u16()) || attempt == self.retries {
                return Err(error);
            }
            last_error = Some(error);
            portable_sleep(retry_delay(retry_after.as_deref(), attempt)).await;
        }

        Err(last_error.unwrap_or_else(|| XReadError::upstream(format!("{label}失败。"))))
    }
}

impl Default for XReader {
    fn default() -> Self {
        Self::new(ReaderConfig::default()).expect("default reader configuration is valid")
    }
}

struct Conversation {
    post: crate::Post,
    replies: Vec<crate::Post>,
    available_count: Option<u64>,
    count: usize,
    truncated: bool,
}

fn missing_community_post() -> XReadError {
    XReadError::invalid_response("FxTwitter 没有返回目标推文。")
        .with_hint("推文可能不存在、已删除、受保护，或第三方服务暂时异常。")
}

fn combined_source_error(community: &XReadError, oembed: &XReadError) -> XReadError {
    XReadError::upstream(format!(
        "免费结构化源与 oEmbed 都无法读取该推文：{}；{}",
        community.message, oembed.message
    ))
    .with_hint("推文可能已删除、受保护，或两个公开服务暂时不可用。")
}

fn json_array(value: Option<&Value>) -> impl Iterator<Item = &Value> {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
}

fn json_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|number| number.is_finite() && *number >= 0.0)
            .map(|number| number as u64)
    })
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn upstream_http_error(label: &str, status: u16, body: &str) -> XReadError {
    let payload = serde_json::from_str::<Value>(body).ok();
    let details = payload
        .as_ref()
        .map(format_error_details)
        .filter(|details| !details.is_empty())
        .unwrap_or_else(|| body.trim().chars().take(180).collect());
    let suffix = if details.is_empty() {
        String::new()
    } else {
        format!("：{details}")
    };
    let mut error = XReadError::upstream(format!("{label}失败（HTTP {status}）{suffix}"))
        .with_upstream_status(status);
    error.hint = match status {
        404 => Some("推文可能不存在、已删除、受保护，或链接不可嵌入。".to_owned()),
        429 => Some("上游触发速率限制，请稍后重试。".to_owned()),
        _ => None,
    };
    error
}

fn format_error_details(value: &Value) -> String {
    if let Some(errors) = value.get("errors") {
        return format_error_details(errors);
    }
    match value {
        Value::Array(items) => items
            .iter()
            .map(format_error_details)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join("；"),
        Value::Object(object) => ["detail", "message", "title"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned(),
        Value::String(value) => value.clone(),
        _ => String::new(),
    }
}

fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500..=599)
}

fn retry_delay(retry_after: Option<&str>, attempt: u8) -> Duration {
    if let Some(seconds) = retry_after.and_then(|value| value.parse::<f64>().ok()) {
        return Duration::from_millis((seconds.max(0.0) * 1_000.0).min(5_000.0) as u64);
    }
    Duration::from_millis((400_u64.saturating_mul(2_u64.pow(u32::from(attempt)))).min(2_000))
}

#[cfg(target_arch = "wasm32")]
async fn portable_sleep(duration: Duration) {
    worker::Delay::from(duration).await;
}

#[cfg(not(target_arch = "wasm32"))]
async fn portable_sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_policy_is_bounded() {
        assert!(retryable_status(429));
        assert!(retryable_status(503));
        assert!(!retryable_status(404));
        assert_eq!(retry_delay(None, 0), Duration::from_millis(400));
        assert_eq!(retry_delay(None, 5), Duration::from_millis(2_000));
        assert_eq!(retry_delay(Some("9"), 0), Duration::from_millis(5_000));
    }
}
