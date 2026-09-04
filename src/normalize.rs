use std::{collections::HashMap, sync::LazyLock};

use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde_json::Value;
use url::Url;

use crate::{Article, Author, Link, Media, MediaVariant, Metrics, Post, XReadError};

static PARAGRAPH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<p\b[^>]*>(.*?)</p>").expect("valid paragraph regex"));
static ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<a\b[^>]*href\s*=\s*["']([^"']*)["'][^>]*>(.*?)</a>"#)
        .expect("valid anchor regex")
});
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<[^>]+>").expect("valid HTML tag regex"));
static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").expect("valid break regex"));
static LANG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<p\b[^>]*\blang=["']([^"']+)["']"#).expect("valid lang regex")
});
static URL_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)https?://[^\s)>\]]+").expect("valid inline URL regex"));

pub(crate) fn is_community_status(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("id").is_none_or(|id| id.as_str().is_none()) {
        return false;
    }
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "status")
    {
        return false;
    }
    let has_text = object.get("text").and_then(Value::as_str).is_some()
        || object
            .get("raw_text")
            .and_then(|raw| raw.get("text"))
            .and_then(Value::as_str)
            .is_some();
    has_text && object.get("author").is_some_and(Value::is_object)
}

/// Convert FxTwitter's deliberately loose JSON into our stable public model.
/// `serde_json::Value` is intentional at this boundary: the third-party schema
/// evolves frequently, while everything returned from this function is typed.
pub(crate) fn normalize_community_post(value: &Value, depth: u8) -> Result<Post, XReadError> {
    if !is_community_status(value) {
        return Err(XReadError::invalid_response(
            "FxTwitter 返回了无法识别的推文数据。",
        ));
    }

    let author_value = &value["author"];
    let author = normalize_author(author_value);
    let username = author.username.as_deref();
    let id = string(value.get("id"));
    let article = normalize_article(value.get("article"));
    let raw_text = value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| path_str(value, &["raw_text", "text"]))
        .unwrap_or_default();
    let expanded_text = expand_community_text(raw_text, value.get("raw_text"));
    let text =
        if article.is_some() && (expanded_text.trim().is_empty() || is_only_urls(&expanded_text)) {
            article
                .as_ref()
                .map(|article| article.text.clone())
                .unwrap_or_default()
        } else {
            expanded_text
        };
    let quoted_post = if depth < 2 && value.get("quote").is_some_and(is_community_status) {
        Some(Box::new(normalize_community_post(
            &value["quote"],
            depth + 1,
        )?))
    } else {
        None
    };
    let reposted_by = value
        .get("reposted_by")
        .filter(|item| item.is_object())
        .map(normalize_author);

    let created_at = value
        .get("created_timestamp")
        .and_then(Value::as_f64)
        .and_then(|seconds| timestamp_to_iso((seconds * 1_000.0) as i64))
        .or_else(|| {
            value
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(parse_date)
        })
        .or_else(|| id.as_deref().and_then(snowflake_timestamp));
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .map(normalize_x_url)
        .or_else(|| {
            id.as_ref()
                .map(|id| format!("https://x.com/{}/status/{id}", username.unwrap_or("i")))
        });
    let kind = if reposted_by.is_some() {
        "repost"
    } else if article.is_some() {
        "article"
    } else if quoted_post.is_some() {
        "quote"
    } else if value
        .get("is_note_tweet")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "long-post"
    } else {
        "post"
    };

    Ok(Post {
        article,
        author,
        conversation_id: None,
        created_at,
        date_label: None,
        id,
        is_long_post: value
            .get("is_note_tweet")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        kind: kind.to_owned(),
        lang: string(value.get("lang")),
        links: normalize_community_links(value),
        media: normalize_community_media(value.get("media")),
        metrics: Some(Metrics {
            bookmark_count: number(value.get("bookmarks")),
            impression_count: number(value.get("views")),
            like_count: number(value.get("likes")),
            quote_count: number(value.get("quotes")),
            reply_count: number(value.get("replies")),
            retweet_count: number(value.get("reposts")),
        }),
        parent_id: path_string(value, &["replying_to", "status"]),
        possibly_sensitive: value.get("possibly_sensitive").and_then(Value::as_bool),
        quoted_post,
        reposted_by,
        reposted_post: None,
        source: "fxtwitter-community".to_owned(),
        text,
        url,
    })
}

fn normalize_author(value: &Value) -> Author {
    Author {
        id: string(value.get("id")),
        name: string(value.get("name")),
        profile_image_url: string(value.get("avatar_url")),
        username: string(value.get("screen_name")),
        verified: value
            .get("verification")
            .and_then(|verification| verification.get("verified"))
            .and_then(Value::as_bool),
    }
}

fn normalize_community_media(value: Option<&Value>) -> Vec<Media> {
    let Some(items) = value
        .and_then(|media| media.get("all"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    items
        .iter()
        .map(|item| {
            let formats = item
                .get("formats")
                .and_then(Value::as_array)
                .or_else(|| item.get("variants").and_then(Value::as_array));
            let variants = formats
                .into_iter()
                .flatten()
                .filter_map(|variant| {
                    Some(MediaVariant {
                        bit_rate: number(
                            variant.get("bitrate").or_else(|| variant.get("bit_rate")),
                        ),
                        content_type: string(
                            variant
                                .get("contentType")
                                .or_else(|| variant.get("content_type"))
                                .or_else(|| variant.get("container")),
                        ),
                        url: variant.get("url")?.as_str()?.to_owned(),
                    })
                })
                .collect();
            let mosaic_url = item
                .get("formats")
                .and_then(Value::as_object)
                .and_then(|formats| {
                    formats
                        .get("jpeg")
                        .or_else(|| formats.get("webp"))
                        .and_then(Value::as_str)
                });
            let duration_ms = number(item.get("duration_ms")).or_else(|| {
                item.get("duration")
                    .and_then(Value::as_f64)
                    .map(|seconds| (seconds * 1_000.0) as u64)
            });

            Media {
                alt_text: string(item.get("altText").or_else(|| item.get("alt_text"))),
                duration_ms,
                height: number(item.get("height")),
                preview_image_url: string(
                    item.get("thumbnailUrl")
                        .or_else(|| item.get("thumbnail_url")),
                ),
                media_type: string(item.get("type")),
                url: item
                    .get("url")
                    .and_then(Value::as_str)
                    .or(mosaic_url)
                    .or_else(|| item.get("thumbnailUrl").and_then(Value::as_str))
                    .or_else(|| item.get("thumbnail_url").and_then(Value::as_str))
                    .map(str::to_owned),
                variants,
                width: number(item.get("width")),
            }
        })
        .collect()
}

fn normalize_community_links(value: &Value) -> Vec<Link> {
    let Some(facets) = value
        .get("raw_text")
        .and_then(|raw| raw.get("facets"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut links = Vec::new();
    for facet in facets {
        if facet
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "url")
        {
            continue;
        }
        let Some(href) = facet
            .get("replacement")
            .or_else(|| facet.get("expanded_url"))
            .or_else(|| facet.get("url"))
            .and_then(Value::as_str)
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        else {
            continue;
        };
        let href = normalize_x_url(href);
        if links.iter().any(|link: &Link| link.href == href) {
            continue;
        }
        links.push(Link {
            text: facet
                .get("display")
                .and_then(Value::as_str)
                .unwrap_or(&href)
                .to_owned(),
            href,
        });
    }
    links
}

/// Replace `t.co` tokens in the visible post text with their final URL. The
/// exact facet shape is upstream-owned, so we accept the field names seen in
/// both older and current FxTwitter responses at this normalization boundary.
fn expand_community_text(value: &str, raw_text: Option<&Value>) -> String {
    let mut text = value.to_owned();
    for facet in raw_text
        .and_then(|raw| raw.get("facets"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if facet
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "url")
        {
            continue;
        }
        let replacement = facet
            .get("replacement")
            .or_else(|| facet.get("expanded_url"))
            .or_else(|| facet.get("unwound_url"))
            .and_then(Value::as_str);
        let source = facet
            .get("original")
            .or_else(|| facet.get("short_url"))
            .or_else(|| facet.get("source"))
            .or_else(|| facet.get("url"))
            .and_then(Value::as_str)
            .filter(|candidate| text.contains(candidate));
        if let (Some(source), Some(replacement)) = (source, replacement) {
            text = text.replace(source, replacement);
        }
    }
    text
}

fn normalize_article(value: Option<&Value>) -> Option<Article> {
    let article = value?.as_object()?;
    let title = non_empty_string(article.get("title"));
    let content = article
        .get("content")
        .or_else(|| article.get("content_state"));
    let blocks = content
        .and_then(|content| content.get("blocks"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let entity_map = normalize_entity_map(content.and_then(|content| content.get("entityMap")));
    let media_entities = article
        .get("media_entities")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let article_media = normalize_article_media_entries(media_entities);
    let rendered = render_article_blocks(blocks, &entity_map, &article_media);
    let fallback = non_empty_string(article.get("plain_text"))
        .or_else(|| non_empty_string(article.get("text")))
        .or_else(|| non_empty_string(article.get("preview_text")))
        .unwrap_or_default();
    let entity_links = normalize_entity_links(article.get("entities"));
    let text = if rendered.text.is_empty() {
        expand_text_with_entity_links(&fallback, article.get("entities"))
    } else {
        rendered.text
    };
    let links = unique_links(rendered.links.into_iter().chain(entity_links));
    let id = string(article.get("id").or_else(|| article.get("rest_id")));
    if title.is_none() && text.is_empty() && id.is_none() {
        return None;
    }
    let markdown = if rendered.markdown.is_empty() {
        text.clone()
    } else {
        rendered.markdown
    };

    Some(Article {
        cover_media_url: article_cover_url(article.get("cover_media")),
        created_at: article
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(parse_date)
            .or_else(|| {
                article
                    .get("metadata")
                    .and_then(|metadata| metadata.get("first_published_at_secs"))
                    .and_then(Value::as_f64)
                    .and_then(|seconds| timestamp_to_iso((seconds * 1_000.0) as i64))
            }),
        embedded_post_ids: rendered.embedded_post_ids,
        id,
        links,
        markdown,
        media: article_media
            .iter()
            .map(|entry| entry.media.clone())
            .collect(),
        modified_at: article
            .get("modified_at")
            .and_then(Value::as_str)
            .and_then(parse_date)
            .or_else(|| {
                article
                    .get("lifecycle_state")
                    .and_then(|state| state.get("modified_at_secs"))
                    .and_then(Value::as_f64)
                    .and_then(|seconds| timestamp_to_iso((seconds * 1_000.0) as i64))
            }),
        preview_text: non_empty_string(article.get("preview_text")),
        text,
        title,
    })
}

struct RenderedArticle {
    embedded_post_ids: Vec<String>,
    links: Vec<Link>,
    markdown: String,
    text: String,
}

struct ArticleMediaEntry {
    ids: Vec<String>,
    media: Media,
}

fn normalize_article_media_entries(values: &[Value]) -> Vec<ArticleMediaEntry> {
    let mut entries = Vec::new();
    for value in values {
        let Some(media) = normalize_article_media(value) else {
            continue;
        };
        let ids = ["id", "id_str", "media_id", "media_key"]
            .into_iter()
            .filter_map(|key| value.get(key).and_then(value_as_string))
            .collect();
        entries.push(ArticleMediaEntry { ids, media });
    }
    entries
}

fn normalize_article_media(value: &Value) -> Option<Media> {
    let source_url = article_media_url(value);
    let video_info = value.get("video_info").or_else(|| {
        value
            .get("media_info")
            .and_then(|info| info.get("video_info"))
    });
    let variants = video_info
        .and_then(|info| info.get("variants"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|variant| {
            Some(MediaVariant {
                bit_rate: number(variant.get("bitrate").or_else(|| variant.get("bit_rate"))),
                content_type: string(
                    variant
                        .get("content_type")
                        .or_else(|| variant.get("contentType"))
                        .or_else(|| variant.get("container")),
                ),
                url: variant.get("url")?.as_str()?.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    let raw_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(if variants.is_empty() {
            "photo"
        } else {
            "video"
        });
    let media_type = match raw_type {
        "image" => "photo",
        "animated_gif" => "gif",
        other => other,
    }
    .to_owned();
    let is_video = matches!(media_type.as_str(), "video" | "gif");
    let direct_mp4 = source_url
        .as_deref()
        .is_some_and(|url| url.to_ascii_lowercase().contains(".mp4"));
    if source_url.is_none() && variants.is_empty() {
        return None;
    }

    Some(Media {
        alt_text: non_empty_string(
            value
                .get("alt_text")
                .or_else(|| value.get("ext_alt_text"))
                .or_else(|| {
                    value
                        .get("media_info")
                        .and_then(|info| info.get("alt_text"))
                }),
        ),
        duration_ms: number(
            value
                .get("duration_ms")
                .or_else(|| value.get("duration_millis"))
                .or_else(|| video_info.and_then(|info| info.get("duration_millis"))),
        ),
        height: number(value.get("height").or_else(|| {
            value.get("media_info").and_then(|info| {
                info.get("original_img_height")
                    .or_else(|| info.get("height"))
            })
        })),
        preview_image_url: is_video.then(|| source_url.clone()).flatten(),
        media_type: Some(media_type),
        url: if is_video && !direct_mp4 {
            None
        } else {
            source_url
        },
        variants,
        width: number(value.get("width").or_else(|| {
            value
                .get("media_info")
                .and_then(|info| info.get("original_img_width").or_else(|| info.get("width")))
        })),
    })
}

fn normalize_entity_map(value: Option<&Value>) -> HashMap<String, Value> {
    let Some(value) = value else {
        return HashMap::new();
    };
    if let Some(entries) = value.as_array() {
        return entries
            .iter()
            .filter_map(|entry| {
                let key = entry.get("key")?;
                let key = key
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| key.as_u64().map(|key| key.to_string()))?;
                Some((key, entry.get("value")?.clone()))
            })
            .collect();
    }
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn render_article_blocks(
    blocks: &[Value],
    entity_map: &HashMap<String, Value>,
    media_entries: &[ArticleMediaEntry],
) -> RenderedArticle {
    let mut markdown_lines = Vec::new();
    let mut text_lines = Vec::new();
    let mut links = Vec::new();
    let mut embedded_post_ids = Vec::new();
    let mut media_by_id = HashMap::new();
    for entry in media_entries {
        for id in &entry.ids {
            media_by_id.insert(id.as_str(), &entry.media);
        }
    }

    for block in blocks {
        let kind = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unstyled");
        let raw = block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_end();
        if kind == "atomic" {
            for range in block
                .get("entityRanges")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(entity) = entity_for_range(range, entity_map) else {
                    continue;
                };
                let entity_type = entity.get("type").and_then(Value::as_str);
                let data = entity.get("data").unwrap_or(&Value::Null);
                if entity_type == Some("TWEET") {
                    if let Some(id) = data.get("tweetId").and_then(value_as_string) {
                        if !embedded_post_ids.contains(&id) {
                            let url = format!("https://x.com/i/status/{id}");
                            markdown_lines.push(format!("<{url}>"));
                            text_lines.push(url);
                            embedded_post_ids.push(id);
                        }
                    }
                } else if entity_type == Some("MEDIA") {
                    let mut media_ids = data
                        .get("mediaItems")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|item| {
                            item.get("mediaId")
                                .or_else(|| item.get("localMediaId"))
                                .and_then(value_as_string)
                        })
                        .collect::<Vec<_>>();
                    // Some Article payloads put the media id directly on the
                    // entity instead of wrapping it in `mediaItems`.
                    if let Some(media_id) = data
                        .get("mediaId")
                        .or_else(|| data.get("localMediaId"))
                        .and_then(value_as_string)
                        .filter(|media_id| !media_ids.contains(media_id))
                    {
                        media_ids.push(media_id);
                    }
                    for media_id in media_ids {
                        if let Some(media) = media_by_id.get(media_id.as_str()) {
                            if let Some(markdown) = article_media_markdown(media) {
                                markdown_lines.push(markdown);
                            }
                            if let Some(text) = article_media_text(media) {
                                text_lines.push(text);
                            }
                        }
                    }
                }
            }
            continue;
        }
        if raw.is_empty() && kind != "unstyled" {
            continue;
        }

        let inline = render_article_inline(block, entity_map);
        for link in &inline.links {
            if !links
                .iter()
                .any(|existing: &Link| existing.href == link.href)
            {
                links.push(link.clone());
            }
        }
        let body = inline.markdown;
        let depth = block.get("depth").and_then(Value::as_u64).unwrap_or(0) as usize;
        let list_indent = "  ".repeat(depth);
        let markdown = match kind {
            "header-one" => format!("# {body}"),
            "header-two" => format!("## {body}"),
            "header-three" => format!("### {body}"),
            "unordered-list-item" => format!("{list_indent}- {body}"),
            "ordered-list-item" => format!("{list_indent}1. {body}"),
            "blockquote" => format!("> {body}"),
            "code-block" => format!("    {body}"),
            _ => body,
        };
        markdown_lines.push(markdown);
        text_lines.push(raw.to_owned());
    }

    RenderedArticle {
        embedded_post_ids,
        links,
        markdown: collapse_blank_lines(&markdown_lines.join("\n\n")),
        text: collapse_blank_lines(&text_lines.join("\n\n")),
    }
}

fn article_media_markdown(media: &Media) -> Option<String> {
    if matches!(media.media_type.as_deref(), Some("video" | "gif")) {
        return media.video_url().map(str::to_owned);
    }
    let url = media.url.as_deref()?;
    let alt = media
        .alt_text
        .as_deref()
        .map(escape_markdown_label)
        .unwrap_or_default();
    Some(format!("![{alt}](<{url}>)"))
}

fn article_media_text(media: &Media) -> Option<String> {
    if matches!(media.media_type.as_deref(), Some("video" | "gif")) {
        media.video_url().map(str::to_owned)
    } else {
        media.url.clone()
    }
}

fn escape_markdown_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

struct RenderedInline {
    links: Vec<Link>,
    markdown: String,
}

struct InlineLink {
    end: usize,
    href: String,
    start: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArticleInlineStyle {
    Bold,
    Italic,
    Strikethrough,
    Code,
}

impl ArticleInlineStyle {
    fn markers(self) -> (&'static str, &'static str) {
        match self {
            Self::Bold => ("**", "**"),
            // Underscores avoid ambiguous runs such as ***** when bold and
            // italic ranges start or end at the same DraftJS offset.
            Self::Italic => ("_", "_"),
            Self::Strikethrough => ("~~", "~~"),
            Self::Code => ("`", "`"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct InlineStyleRange {
    end: usize,
    start: usize,
    style: ArticleInlineStyle,
}

/// Render DraftJS entity ranges in place. Offsets are UTF-16 code units, so we
/// build the output from UTF-16 slices rather than indexing a Rust UTF-8 string.
fn render_article_inline(block: &Value, entity_map: &HashMap<String, Value>) -> RenderedInline {
    let text = block
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end();
    let units = text.encode_utf16().collect::<Vec<_>>();
    let mut links: Vec<Link> = extract_urls(text)
        .into_iter()
        .map(|href| Link {
            text: href.clone(),
            href,
        })
        .collect();
    let style_ranges = article_inline_style_ranges(block, units.len());
    let mut inline_links = Vec::new();
    for range in block
        .get("entityRanges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(entity) = entity_for_range(range, entity_map) else {
            continue;
        };
        if matches!(
            entity.get("type").and_then(Value::as_str),
            Some("TWEET" | "MEDIA")
        ) {
            continue;
        }
        let Some(data) = entity.get("data") else {
            continue;
        };
        let Some(href) = ["url", "href", "markdown", "destinationUrl"]
            .into_iter()
            .filter_map(|key| data.get(key).and_then(Value::as_str))
            .flat_map(extract_urls)
            .next()
        else {
            continue;
        };
        let start = range.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let length = range.get("length").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(end) = start.checked_add(length).filter(|end| *end <= units.len()) else {
            continue;
        };
        let Some(label) = utf16_slice(text, start, length) else {
            continue;
        };
        if !links.iter().any(|link| link.href == href) {
            links.push(Link {
                href: href.clone(),
                text: label.clone(),
            });
        }
        inline_links.push(InlineLink { end, href, start });
    }
    inline_links.sort_by_key(|link| link.start);

    let mut markdown = String::new();
    let mut cursor = 0;
    for link in inline_links {
        // Overlapping entity ranges are invalid DraftJS. Keeping the original
        // text is safer than generating broken, nested Markdown links.
        if link.start < cursor {
            continue;
        }
        markdown.push_str(&render_styled_utf16_range(
            &units,
            cursor,
            link.start,
            &style_ranges,
            false,
        ));
        let label = render_styled_utf16_range(&units, link.start, link.end, &style_ranges, true);
        markdown.push_str(&format!("[{}](<{}>)", label, link.href));
        cursor = link.end;
    }
    markdown.push_str(&render_styled_utf16_range(
        &units,
        cursor,
        units.len(),
        &style_ranges,
        false,
    ));

    RenderedInline { links, markdown }
}

fn article_inline_style_ranges(block: &Value, text_len: usize) -> Vec<InlineStyleRange> {
    block
        .get("inlineStyleRanges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|range| {
            let style = match range.get("style").and_then(Value::as_str)? {
                "BOLD" => ArticleInlineStyle::Bold,
                "ITALIC" => ArticleInlineStyle::Italic,
                "STRIKETHROUGH" => ArticleInlineStyle::Strikethrough,
                "CODE" | "MONOSPACE" => ArticleInlineStyle::Code,
                // DraftJS may contain presentation-only styles such as color
                // or font size. Markdown has no portable equivalent, so the
                // visible text is preserved without inventing HTML.
                _ => return None,
            };
            let start = range.get("offset")?.as_u64()? as usize;
            let length = range.get("length")?.as_u64()? as usize;
            let end = start.checked_add(length)?;
            (start < end && end <= text_len).then_some(InlineStyleRange { end, start, style })
        })
        .collect()
}

/// Rebuild the common DraftJS inline styles while respecting UTF-16 offsets.
///
/// Ranges can overlap. We split only at their boundaries, then close and reopen
/// Markdown markers in a stable order. This produces valid nesting even for a
/// crossing pair such as bold 0..10 plus italic 5..15.
fn render_styled_utf16_range(
    units: &[u16],
    start: usize,
    end: usize,
    ranges: &[InlineStyleRange],
    escape_as_link_label: bool,
) -> String {
    if start >= end || end > units.len() {
        return String::new();
    }

    let mut boundaries = vec![start, end];
    for range in ranges {
        if range.start < end && range.end > start {
            boundaries.push(range.start.max(start));
            boundaries.push(range.end.min(end));
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut output = String::new();
    let mut active: Vec<ArticleInlineStyle> = Vec::new();
    for window in boundaries.windows(2) {
        let segment_start = window[0];
        let segment_end = window[1];
        let mut target = [
            ArticleInlineStyle::Bold,
            ArticleInlineStyle::Italic,
            ArticleInlineStyle::Strikethrough,
            ArticleInlineStyle::Code,
        ]
        .into_iter()
        .filter(|style| {
            ranges.iter().any(|range| {
                range.style == *style && range.start <= segment_start && range.end >= segment_end
            })
        })
        .collect::<Vec<_>>();
        // Markdown inside an inline-code span is literal. Keeping only CODE is
        // closer to DraftJS than printing visible ** or ~~ characters.
        if target.contains(&ArticleInlineStyle::Code) {
            target.retain(|style| *style == ArticleInlineStyle::Code);
        }

        let common = active
            .iter()
            .zip(&target)
            .take_while(|(left, right)| left == right)
            .count();
        for style in active[common..].iter().rev() {
            output.push_str(style.markers().1);
        }
        for style in &target[common..] {
            output.push_str(style.markers().0);
        }
        active = target;

        if let Ok(mut text) = String::from_utf16(&units[segment_start..segment_end]) {
            if escape_as_link_label {
                text = escape_markdown_label(&text);
            }
            output.push_str(&text);
        }
    }
    for style in active.iter().rev() {
        output.push_str(style.markers().1);
    }
    output
}

fn entity_for_range<'a>(
    range: &Value,
    entity_map: &'a HashMap<String, Value>,
) -> Option<&'a Value> {
    let key = range.get("key").and_then(value_as_string)?;
    entity_map.get(&key)
}

/// DraftJS ranges use JavaScript UTF-16 offsets, not Rust UTF-8 byte offsets.
/// Decoding the selected units avoids panics and keeps labels correct around emoji.
fn utf16_slice(value: &str, offset: usize, length: usize) -> Option<String> {
    let units: Vec<u16> = value.encode_utf16().collect();
    let end = offset.checked_add(length)?;
    String::from_utf16(units.get(offset..end)?).ok()
}

fn normalize_entity_links(value: Option<&Value>) -> Vec<Link> {
    value
        .and_then(|entities| entities.get("urls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let href = item
                .get("expanded_url")
                .or_else(|| item.get("unwound_url"))
                .or_else(|| item.get("url"))
                .and_then(Value::as_str)?;
            Some(Link {
                href: href.to_owned(),
                text: item
                    .get("display_url")
                    .and_then(Value::as_str)
                    .unwrap_or(href)
                    .to_owned(),
            })
        })
        .collect()
}

fn expand_text_with_entity_links(value: &str, entities: Option<&Value>) -> String {
    let mut text = value.to_owned();
    for item in entities
        .and_then(|entities| entities.get("urls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let source = item.get("url").and_then(Value::as_str);
        let replacement = item
            .get("expanded_url")
            .or_else(|| item.get("unwound_url"))
            .and_then(Value::as_str);
        if let (Some(source), Some(replacement)) = (source, replacement) {
            text = text.replace(source, replacement);
        }
    }
    text
}

fn unique_links(links: impl IntoIterator<Item = Link>) -> Vec<Link> {
    let mut unique = Vec::new();
    for link in links {
        if !unique.iter().any(|item: &Link| item.href == link.href) {
            unique.push(link);
        }
    }
    unique
}

fn extract_urls(value: &str) -> Vec<String> {
    URL_IN_TEXT
        .find_iter(value)
        .map(|found| found.as_str().to_owned())
        .collect()
}

fn article_cover_url(value: Option<&Value>) -> Option<String> {
    let cover = value?;
    [
        path_str(cover, &["media_info", "original_img_url"]),
        path_str(cover, &["media_info", "media_url_https"]),
        path_str(cover, &["media_info", "media_url"]),
        cover.get("url").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .next()
    .map(str::to_owned)
}

fn article_media_url(value: &Value) -> Option<String> {
    [
        path_str(value, &["media_info", "original_img_url"]),
        path_str(value, &["media_info", "media_url_https"]),
        path_str(value, &["media_info", "media_url"]),
        value.get("media_url_https").and_then(Value::as_str),
        value.get("media_url").and_then(Value::as_str),
        value.get("url").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .next()
    .map(str::to_owned)
}

pub(crate) fn normalize_oembed(payload: &Value, id: &str) -> Result<Post, XReadError> {
    let html = payload.get("html").and_then(Value::as_str).ok_or_else(|| {
        XReadError::invalid_response("X oEmbed 返回了无法识别的数据。")
            .with_hint("推文可能已删除、受保护或禁止嵌入。")
    })?;
    let paragraph = PARAGRAPH
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str())
        .ok_or_else(|| {
            XReadError::invalid_response("X oEmbed 响应里没有找到推文正文。")
                .with_hint("X 可能修改了嵌入格式。")
        })?;
    let links = extract_anchors(paragraph);
    let date_label = extract_anchors(html)
        .into_iter()
        .rfind(|anchor| {
            anchor.href.contains("/status/")
                && anchor
                    .href
                    .split("/status/")
                    .nth(1)
                    .is_some_and(|tail| tail.chars().next().is_some_and(|c| c.is_ascii_digit()))
        })
        .map(|anchor| anchor.text);
    let username = payload
        .get("author_url")
        .and_then(Value::as_str)
        .and_then(|value| Url::parse(value).ok())
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.find(|part| !part.is_empty()).map(str::to_owned))
        });
    let canonical_url = payload
        .get("url")
        .and_then(Value::as_str)
        .map(normalize_x_url)
        .unwrap_or_else(|| {
            format!(
                "https://x.com/{}/status/{id}",
                username.as_deref().unwrap_or("i")
            )
        });

    Ok(Post {
        article: None,
        author: Author {
            id: None,
            name: string(payload.get("author_name")),
            profile_image_url: None,
            username,
            verified: None,
        },
        conversation_id: None,
        created_at: snowflake_timestamp(id),
        date_label,
        id: Some(id.to_owned()),
        is_long_post: false,
        kind: "post".to_owned(),
        lang: LANG
            .captures(html)
            .and_then(|captures| captures.get(1))
            .map(|capture| capture.as_str().to_owned()),
        links,
        media: Vec::new(),
        metrics: None,
        parent_id: None,
        possibly_sensitive: None,
        quoted_post: None,
        reposted_by: None,
        reposted_post: None,
        source: "x-oembed".to_owned(),
        text: html_to_text(paragraph),
        url: Some(canonical_url),
    })
}

fn extract_anchors(html: &str) -> Vec<Link> {
    ANCHOR
        .captures_iter(html)
        .filter_map(|captures| {
            Some(Link {
                href: html_escape::decode_html_entities(captures.get(1)?.as_str()).into_owned(),
                text: html_to_text(captures.get(2)?.as_str()),
            })
        })
        .collect()
}

fn html_to_text(html: &str) -> String {
    let with_breaks = BREAK.replace_all(html, "\n");
    let without_tags = TAG.replace_all(&with_breaks, "");
    html_escape::decode_html_entities(&without_tags)
        .replace('\u{a0}', " ")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

pub fn snowflake_timestamp(id: &str) -> Option<String> {
    if !(15..=19).contains(&id.len()) || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let snowflake = id.parse::<u64>().ok()?;
    let milliseconds = (snowflake >> 22).checked_add(1_288_834_974_657)?;
    timestamp_to_iso(i64::try_from(milliseconds).ok()?)
}

fn parse_date(value: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .or_else(|| DateTime::parse_from_str(value, "%a %b %d %H:%M:%S %z %Y").ok())
        .map(|date| {
            date.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
}

fn timestamp_to_iso(milliseconds: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(milliseconds)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn is_only_urls(value: &str) -> bool {
    let mut parts = value.split_whitespace().peekable();
    parts.peek().is_some()
        && parts
            .all(|part| Url::parse(part).is_ok_and(|url| matches!(url.scheme(), "http" | "https")))
}

fn collapse_blank_lines(value: &str) -> String {
    let mut output = String::new();
    let mut blank = false;
    for line in value.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && blank {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
        blank = is_blank;
    }
    output.trim().to_owned()
}

fn normalize_x_url(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("https://twitter.com") {
        format!("https://x.com{rest}")
    } else {
        value.to_owned()
    }
}

fn path_str<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str()
}

fn path_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    value_as_string(current)
}

fn value_as_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|number| number.to_string()))
}

fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(value_as_string)
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    string(value).filter(|value| !value.trim().is_empty())
}

fn number(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value.as_u64().or_else(|| {
            value
                .as_f64()
                .filter(|number| number.is_finite() && *number >= 0.0)
                .map(|number| number as u64)
        })
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn normalizes_long_post_media_quote_and_links() {
        let value = json!({
            "id": "1890000000000000000",
            "type": "status",
            "text": "hello https://t.co/a",
            "is_note_tweet": true,
            "author": {
                "id": "7",
                "name": "Ada",
                "screen_name": "ada",
                "verification": { "verified": true }
            },
            "raw_text": { "text": "hello https://t.co/a", "facets": [{
                "type": "url", "original": "https://t.co/a",
                "replacement": "https://example.com", "display": "example.com"
            }]},
            "media": { "all": [{
                "type": "video", "thumbnail_url": "https://img.example/1.jpg",
                "variants": [{ "url": "https://video.example/1.mp4", "bitrate": 832000 }]
            }]},
            "likes": 12,
            "replies": 3
        });

        let post = normalize_community_post(&value, 0).unwrap();
        assert_eq!(post.author.username.as_deref(), Some("ada"));
        assert_eq!(post.kind, "long-post");
        assert_eq!(post.text, "hello https://example.com");
        assert_eq!(post.links[0].href, "https://example.com");
        assert_eq!(post.media[0].variants[0].bit_rate, Some(832_000));
        assert_eq!(post.metrics.unwrap().like_count, Some(12));
    }

    #[test]
    fn renders_article_blocks_and_draft_js_link_ranges() {
        let value = json!({
            "id": "1890000000000000000",
            "type": "status",
            "text": "https://t.co/article",
            "author": { "name": "Ada", "screen_name": "ada" },
            "article": {
                "id": "article-1",
                "title": "A small article",
                "content": {
                    "blocks": [
                        { "type": "header-one", "text": "Intro", "entityRanges": [] },
                        {
                            "type": "unstyled",
                            "text": "Go 🚀docs",
                            "entityRanges": [{ "key": 0, "offset": 5, "length": 4 }],
                            "inlineStyleRanges": [{ "style": "BOLD", "offset": 5, "length": 4 }]
                        },
                        {
                            "type": "unstyled",
                            "text": "Bold and italic code",
                            "entityRanges": [],
                            "inlineStyleRanges": [
                                { "style": "BOLD", "offset": 0, "length": 4 },
                                { "style": "ITALIC", "offset": 9, "length": 6 },
                                { "style": "CODE", "offset": 16, "length": 4 }
                            ]
                        }
                    ],
                    "entityMap": {
                        "0": {
                            "type": "LINK",
                            "data": { "url": "https://example.com/docs" }
                        }
                    }
                }
            }
        });

        let post = normalize_community_post(&value, 0).unwrap();
        let article = post.article.unwrap();
        assert_eq!(post.kind, "article");
        assert_eq!(article.title.as_deref(), Some("A small article"));
        assert!(article.markdown.starts_with("# Intro"));
        assert!(article
            .markdown
            .contains("Go 🚀[**docs**](<https://example.com/docs>)"));
        assert!(article.markdown.contains("**Bold** and _italic_ `code`"));
        assert!(article.text.contains("Go 🚀docs"));
        assert!(!article.text.contains("https://example.com/docs"));
        assert_eq!(article.links[0].text, "docs");
        assert_eq!(article.links[0].href, "https://example.com/docs");
        assert_eq!(post.text, article.text);
    }

    #[test]
    fn preserves_article_media_without_artificial_labels() {
        let value = json!({
            "id": "1890000000000000000",
            "type": "status",
            "text": "https://t.co/article",
            "author": { "name": "Ada", "screen_name": "ada" },
            "article": {
                "id": "article-1",
                "title": "A visual article",
                "content": {
                    "blocks": [
                        { "type": "unstyled", "text": "Before", "entityRanges": [] },
                        {
                            "type": "atomic",
                            "text": " ",
                            "entityRanges": [{ "key": 0, "offset": 0, "length": 1 }]
                        },
                        { "type": "unstyled", "text": "After", "entityRanges": [] }
                    ],
                    "entityMap": {
                        "0": {
                            "type": "MEDIA",
                            "data": { "mediaItems": [{ "mediaId": "media-1" }] }
                        }
                    }
                },
                "media_entities": [{
                    "id": "media-1",
                    "type": "image",
                    "alt_text": "Architecture [diagram]",
                    "media_info": {
                        "original_img_url": "https://pbs.twimg.com/media/example.jpg",
                        "original_img_width": 1200,
                        "original_img_height": 800
                    }
                }]
            }
        });

        let post = normalize_community_post(&value, 0).unwrap();
        let article = post.article.unwrap();
        assert_eq!(article.media.len(), 1);
        assert_eq!(article.media[0].media_type.as_deref(), Some("photo"));
        assert_eq!(article.media[0].width, Some(1200));
        assert_eq!(article.media[0].height, Some(800));
        assert_eq!(
            article.markdown,
            "Before\n\n![Architecture \\[diagram\\]](<https://pbs.twimg.com/media/example.jpg>)\n\nAfter"
        );
        assert_eq!(
            article.text,
            "Before\n\nhttps://pbs.twimg.com/media/example.jpg\n\nAfter"
        );
        assert!(!article.markdown.contains("文章媒体"));
        assert!(!article.text.contains("文章媒体"));
    }

    #[test]
    fn parses_oembed_and_decodes_html() {
        let payload = json!({
            "author_name": "Ada",
            "author_url": "https://twitter.com/ada",
            "url": "https://twitter.com/ada/status/463440424141459456",
            "html": "<blockquote><p lang=\"en\">Hello &amp; goodbye<br><a href=\"https://example.com\">link</a></p>&mdash; Ada <a href=\"https://twitter.com/ada/status/463440424141459456\">May 5, 2014</a></blockquote>"
        });
        let post = normalize_oembed(&payload, "463440424141459456").unwrap();
        assert_eq!(post.text, "Hello & goodbye\nlink");
        assert_eq!(post.author.username.as_deref(), Some("ada"));
        assert_eq!(post.date_label.as_deref(), Some("May 5, 2014"));
        assert!(post.url.unwrap().starts_with("https://x.com/"));
    }

    #[test]
    fn derives_timestamp_from_snowflake() {
        assert_eq!(
            snowflake_timestamp("463440424141459456").as_deref(),
            Some("2014-05-05T22:09:42.079Z")
        );
    }

    #[test]
    fn slices_draft_js_labels_by_utf16_units() {
        assert_eq!(utf16_slice("a🚀link", 3, 4).as_deref(), Some("link"));
    }
}
