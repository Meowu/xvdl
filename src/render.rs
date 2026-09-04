use std::{fmt, str::FromStr};

use crate::{Author, Link, Media, Metrics, Post, ReadResult, XReadError};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Markdown,
    Text,
    Json,
    Human,
}

impl FromStr for OutputFormat {
    type Err = XReadError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "md" | "markdown" => Ok(Self::Markdown),
            "text" | "txt" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "human" => Ok(Self::Human),
            _ => Err(XReadError::invalid_input(
                "format 只接受 markdown、text、json 或 human。",
            )),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Json => "json",
            Self::Human => "human",
        })
    }
}

/// Compact Markdown intended to be piped directly into an LLM context.
pub fn render_markdown(result: &ReadResult) -> String {
    let mut sections = Vec::new();
    sections.push(markdown_post(&result.post, true));

    if let Some(quoted) = &result.post.quoted_post {
        sections.push(format!(
            "## Quoted post\n\n{}",
            markdown_quote(&markdown_post(quoted, true))
        ));
    }
    if let Some(reposted) = &result.post.reposted_post {
        sections.push(format!(
            "## Reposted content\n\n{}",
            markdown_quote(&markdown_post(reposted, true))
        ));
    }
    if let Some(info) = &result.reply_info {
        let partial = if info.truncated { ", partial" } else { "" };
        let mut replies = vec![format!(
            "## Replies ({} shown{partial})",
            result.replies.len()
        )];
        if result.replies.is_empty() {
            replies.push("No visible replies were returned.".to_owned());
        }
        for reply in &result.replies {
            let label = [
                Some(markdown_author(&reply.author)),
                compact_date(reply),
                compact_media_summary(&reply.media),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let heading = reply
                .url
                .as_ref()
                .map(|url| format!("### [{label}]({url})"))
                .unwrap_or_else(|| format!("### {label}"));
            replies.push(format!("{heading}\n\n{}", markdown_post(reply, false)));
        }
        if info.error.is_some() {
            replies
                .push("> Note: replies were unavailable from the free upstream source.".to_owned());
        }
        sections.push(replies.join("\n\n"));
    }
    if result.post.source == "x-oembed" {
        sections.push(
            "> Note: the structured source was unavailable; oEmbed fallback content may be incomplete."
                .to_owned(),
        );
    }

    sections
        .into_iter()
        .filter(|section| !section.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Plain content with no terminal decoration or backend diagnostics.
pub fn render_text(result: &ReadResult) -> String {
    let mut sections = vec![plain_body(&result.post)];
    if let Some(quoted) = &result.post.quoted_post {
        sections.push(format!("Quoted post:\n{}", plain_body(quoted)));
    }
    if let Some(reposted) = &result.post.reposted_post {
        sections.push(format!("Reposted content:\n{}", plain_body(reposted)));
    }
    if let Some(info) = &result.reply_info {
        let partial = if info.truncated { ", partial" } else { "" };
        let mut replies = vec![format!(
            "Replies ({} shown{partial}):",
            result.replies.len()
        )];
        replies.extend(
            result
                .replies
                .iter()
                .map(|reply| format!("{}:\n{}", render_author(&reply.author), plain_body(reply))),
        );
        sections.push(replies.join("\n\n"));
    }
    sections
        .into_iter()
        .filter(|section| !section.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn markdown_post(post: &Post, include_provenance: bool) -> String {
    let mut blocks = vec![markdown_body(post)];
    let body = blocks[0].clone();
    let mut missing_links = Vec::new();
    for link in post.links.iter().chain(
        post.article
            .as_ref()
            .into_iter()
            .flat_map(|article| article.links.iter()),
    ) {
        if !body.contains(&link.href)
            && !missing_links
                .iter()
                .any(|existing: &&Link| existing.href == link.href)
        {
            missing_links.push(link);
        }
    }
    if !missing_links.is_empty() {
        let links = missing_links
            .iter()
            .map(|link| {
                let label = if link.text.trim().is_empty() {
                    &link.href
                } else {
                    &link.text
                };
                format!("- [{}]({})", escape_markdown(label), link.href)
            })
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(format!("### Links\n\n{links}"));
    }

    let media = compact_media_summary(&post.media);
    let alt_text = post
        .media
        .iter()
        .filter_map(|item| item.alt_text.as_deref())
        .filter(|alt| !alt.trim().is_empty())
        .map(|alt| format!("- Media description: {alt}"))
        .collect::<Vec<_>>();
    if !alt_text.is_empty() {
        blocks.push(alt_text.join("\n"));
    }

    // Image URLs are intentionally omitted from the compact view: they are
    // easy to save from X and four long CDN URLs add substantial noise. Video
    // URLs are much more useful, so expose one best-quality MP4 per video/GIF.
    let video_urls = post.video_urls();
    if !video_urls.is_empty() {
        let videos = video_urls
            .iter()
            .map(|url| format!("- {url}"))
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(format!("### Videos\n\n{videos}"));
    }
    if include_provenance {
        let author = markdown_author(&post.author);
        let date = compact_date(post);
        let label = [Some(author), date, media]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        if let Some(url) = &post.url {
            blocks.push(format!("_[{label}]({url})_"));
        } else if !label.is_empty() {
            blocks.push(format!("_{label}_"));
        }
    }
    blocks.join("\n\n")
}

fn markdown_body(post: &Post) -> String {
    if let Some(article) = &post.article {
        let mut parts = Vec::new();
        if let Some(title) = &article.title {
            parts.push(format!("# {title}"));
        }
        let body = if article.markdown.trim().is_empty() {
            post.text.trim()
        } else {
            article.markdown.trim()
        };
        if let Some(cover) = article
            .cover_media_url
            .as_deref()
            .filter(|cover| !body.contains(cover))
        {
            parts.push(format!("![](<{cover}>)"));
        }
        if !body.is_empty() {
            parts.push(body.to_owned());
        }
        return parts.join("\n\n");
    }
    if post.text.trim().is_empty() {
        "(no text content)".to_owned()
    } else {
        post.text.trim().to_owned()
    }
}

fn plain_body(post: &Post) -> String {
    post.article
        .as_ref()
        .map(|article| article.text.trim())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| post.text.trim())
        .to_owned()
}

fn markdown_quote(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn markdown_author(author: &Author) -> String {
    match (&author.name, &author.username) {
        (Some(name), Some(username)) => format!("{} (@{})", escape_markdown(name), username),
        (Some(name), None) => escape_markdown(name),
        (None, Some(username)) => format!("@{username}"),
        (None, None) => "unknown author".to_owned(),
    }
}

fn compact_date(post: &Post) -> Option<String> {
    post.created_at
        .as_deref()
        .map(|date| date.split('T').next().unwrap_or(date).to_owned())
        .or_else(|| post.date_label.clone())
}

fn compact_media_summary(media: &[Media]) -> Option<String> {
    let photos = media
        .iter()
        .filter(|item| item.media_type.as_deref() == Some("photo"))
        .count();
    let videos = media
        .iter()
        .filter(|item| matches!(item.media_type.as_deref(), Some("video" | "gif")))
        .count();
    let mut parts = Vec::new();
    if photos > 0 {
        parts.push(format!(
            "{photos} image{}",
            if photos == 1 { "" } else { "s" }
        ));
    }
    if videos > 0 {
        parts.push(format!(
            "{videos} video{}",
            if videos == 1 { "" } else { "s" }
        ));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn escape_markdown(value: &str) -> String {
    value.replace('[', "\\[").replace(']', "\\]")
}

/// Render the normalized model for people who explicitly choose `human`.
/// JSON remains the machine contract; this function is intentionally a view.
pub fn render_human(result: &ReadResult) -> String {
    let mut lines = Vec::new();
    let badge = post_badge(&result.post);
    let heading = if badge.is_empty() {
        render_author(&result.post.author)
    } else {
        format!("{} · {badge}", render_author(&result.post.author))
    };
    lines.push(heading);
    if let Some(reposted_by) = &result.post.reposted_by {
        lines.push(format!("转推者：{}", render_author(reposted_by)));
    }
    lines.push(format_date(&result.post));
    lines.push("─".repeat(48));
    lines.push(String::new());
    append_post_content(&mut lines, &result.post);
    append_links(&mut lines, &result.post.links, "链接");
    append_media(&mut lines, &result.post.media);
    if let Some(quoted) = &result.post.quoted_post {
        lines.push(String::new());
        lines.push("引用内容：".to_owned());
        append_nested_post(&mut lines, quoted);
    }
    if let Some(reposted) = &result.post.reposted_post {
        lines.push(String::new());
        lines.push("转推原文：".to_owned());
        append_nested_post(&mut lines, reposted);
    }
    append_metrics(&mut lines, result.post.metrics.as_ref());
    lines.push(String::new());
    lines.push(format!(
        "原帖：{}",
        result.post.url.as_deref().unwrap_or("未知")
    ));
    lines.push(format!("来源：{}", source_label(&result.post.source)));

    if let Some(info) = &result.reply_info {
        let scope = if info.mode == "direct" {
            "直接回复"
        } else {
            "整段对话中的回复"
        };
        let ranking = if info.sort == "recent" {
            "免费最新首批"
        } else {
            "免费精选首批"
        };
        let range = info.available_count.map_or_else(
            || ranking.to_owned(),
            |count| format!("{ranking}，原帖显示共 {} 条回复", format_number(count)),
        );
        let incomplete = if info.truncated {
            "，结果可能不完整"
        } else {
            ""
        };
        lines.push(String::new());
        lines.push(format!(
            "{scope}（{range}，返回 {} 条{incomplete}）",
            result.replies.len()
        ));
        if result.replies.is_empty() {
            lines.push("（没有返回可见回复）".to_owned());
        }
        for (index, reply) in result.replies.iter().enumerate() {
            lines.push(String::new());
            lines.push(format!(
                "[{}] {} · {}",
                index + 1,
                render_author(&reply.author),
                format_date(reply)
            ));
            let body = reply
                .article
                .as_ref()
                .map(|article| article.markdown.as_str())
                .filter(|body| !body.is_empty())
                .or_else(|| (!reply.text.is_empty()).then_some(reply.text.as_str()))
                .unwrap_or("（无文本正文）");
            lines.push(indent(body, "    "));
            if info.mode == "thread" {
                if let Some(parent_id) = &reply.parent_id {
                    lines.push(format!("    ↳ 回复推文 {parent_id}"));
                }
            }
            let metrics = compact_metrics(reply.metrics.as_ref());
            if !metrics.is_empty() {
                lines.push(format!("    {metrics}"));
            }
            for media in &reply.media {
                let suffix = media_url(media)
                    .map(|url| format!(" {url}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "    媒体：{}{}",
                    media.media_type.as_deref().unwrap_or("media"),
                    suffix
                ));
            }
            if let Some(url) = &reply.url {
                lines.push(format!("    {url}"));
            }
        }
    }

    lines.join("\n")
}

fn post_badge(post: &Post) -> &'static str {
    match post.kind.as_str() {
        "article" => "Article",
        "long-post" => "长推文",
        "quote" => "引用推文",
        "repost" => "转推",
        _ if post.is_long_post => "长推文",
        _ => "",
    }
}

fn append_post_content(lines: &mut Vec<String>, post: &Post) {
    if let Some(article) = &post.article {
        if let Some(title) = &article.title {
            lines.push(format!("# {title}"));
        }
        if !article.markdown.is_empty() {
            lines.push(String::new());
            lines.push(article.markdown.clone());
        } else if !post.text.is_empty() {
            lines.push(post.text.clone());
        }
        if let Some(cover) = &article.cover_media_url {
            lines.push(String::new());
            lines.push(format!("封面：{cover}"));
        }
        append_links(lines, &article.links, "文中链接");
    } else {
        lines.push(if post.text.is_empty() {
            "（无文本正文）".to_owned()
        } else {
            post.text.clone()
        });
    }
}

fn append_links(lines: &mut Vec<String>, links: &[Link], title: &str) {
    if links.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("{title}："));
    for (index, link) in links.iter().enumerate() {
        let label = if link.text.is_empty() || link.text == link.href {
            String::new()
        } else {
            format!("{} — ", link.text)
        };
        lines.push(format!("{}. {label}{}", index + 1, link.href));
    }
}

fn append_nested_post(lines: &mut Vec<String>, post: &Post) {
    let mut nested = vec![format!(
        "{} · {}",
        render_author(&post.author),
        format_date(post)
    )];
    if let Some(title) = post
        .article
        .as_ref()
        .and_then(|article| article.title.as_ref())
    {
        nested.push(format!("Article：{title}"));
    }
    nested.push(
        post.article
            .as_ref()
            .map(|article| article.markdown.as_str())
            .filter(|text| !text.is_empty())
            .or_else(|| (!post.text.is_empty()).then_some(post.text.as_str()))
            .unwrap_or("（无文本正文）")
            .to_owned(),
    );
    nested.extend(post.links.iter().map(|link| format!("链接：{}", link.href)));
    if let Some(url) = &post.url {
        nested.push(url.clone());
    }
    lines.push(indent(&nested.join("\n"), "│ "));
}

fn render_author(author: &Author) -> String {
    let username = author
        .username
        .as_ref()
        .map(|username| format!("@{username}"))
        .unwrap_or_else(|| "@未知用户".to_owned());
    author
        .name
        .as_ref()
        .map(|name| format!("{name} ({username})"))
        .unwrap_or(username)
}

fn format_date(post: &Post) -> String {
    post.created_at
        .as_ref()
        .or(post.date_label.as_ref())
        .cloned()
        .unwrap_or_else(|| "时间未知".to_owned())
}

fn append_media(lines: &mut Vec<String>, media: &[Media]) {
    if media.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("媒体：".to_owned());
    for item in media {
        let url = media_url(item)
            .map(|url| format!(": {url}"))
            .unwrap_or_default();
        let alt = item
            .alt_text
            .as_ref()
            .map(|alt| format!("（{alt}）"))
            .unwrap_or_default();
        lines.push(format!(
            "- {}{url}{alt}",
            item.media_type.as_deref().unwrap_or("media")
        ));
    }
}

fn media_url(media: &Media) -> Option<&str> {
    media.url.as_deref().or(media.preview_image_url.as_deref())
}

fn append_metrics(lines: &mut Vec<String>, metrics: Option<&Metrics>) {
    let text = compact_metrics(metrics);
    if !text.is_empty() {
        lines.push(String::new());
        lines.push(text);
    }
}

fn compact_metrics(metrics: Option<&Metrics>) -> String {
    let Some(metrics) = metrics else {
        return String::new();
    };
    [
        (metrics.like_count, "喜欢"),
        (metrics.reply_count, "回复"),
        (metrics.retweet_count, "转帖"),
        (metrics.quote_count, "引用"),
        (metrics.bookmark_count, "书签"),
        (metrics.impression_count, "浏览"),
    ]
    .into_iter()
    .filter_map(|(value, label)| value.map(|value| format!("{label} {}", format_number(value))))
    .collect::<Vec<_>>()
    .join(" · ")
}

fn format_number(value: u64) -> String {
    let reversed: Vec<_> = value.to_string().chars().rev().collect();
    let mut output = String::new();
    for (index, character) in reversed.iter().enumerate() {
        if index > 0 && index % 3 == 0 {
            output.push(',');
        }
        output.push(*character);
    }
    output.chars().rev().collect()
}

fn source_label(source: &str) -> &str {
    match source {
        "fxtwitter-community" => "FxTwitter v2（第三方免费结构化源）",
        "x-oembed" => "X 官方 oEmbed（免登录）",
        _ => source,
    }
}

fn indent(value: &str, prefix: &str) -> String {
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result() -> ReadResult {
        ReadResult {
            post: Post {
                article: None,
                author: Author {
                    name: Some("Ada".to_owned()),
                    username: Some("ada".to_owned()),
                    ..Author::default()
                },
                conversation_id: None,
                created_at: Some("2026-09-03T12:30:00.000Z".to_owned()),
                date_label: None,
                id: Some("123".to_owned()),
                is_long_post: false,
                kind: "post".to_owned(),
                lang: Some("en".to_owned()),
                links: vec![Link {
                    href: "https://example.com".to_owned(),
                    text: "Example".to_owned(),
                }],
                media: vec![Media {
                    alt_text: Some("A red rocket".to_owned()),
                    duration_ms: None,
                    height: Some(800),
                    preview_image_url: Some("https://image.example/1.jpg".to_owned()),
                    media_type: Some("photo".to_owned()),
                    url: Some("https://image.example/1.jpg".to_owned()),
                    variants: Vec::new(),
                    width: Some(600),
                }],
                metrics: Some(Metrics {
                    like_count: Some(99_999),
                    ..Metrics::default()
                }),
                parent_id: None,
                possibly_sensitive: Some(false),
                quoted_post: None,
                reposted_by: None,
                reposted_post: None,
                source: "fxtwitter-community".to_owned(),
                text: "The important body.".to_owned(),
                url: Some("https://x.com/ada/status/123".to_owned()),
            },
            replies: Vec::new(),
            reply_info: None,
            warnings: vec!["an operational warning".to_owned()],
        }
    }

    #[test]
    fn formats_grouped_numbers() {
        assert_eq!(format_number(12), "12");
        assert_eq!(format_number(12_345_678), "12,345,678");
    }

    #[test]
    fn markdown_is_compact_and_keeps_provenance() {
        let markdown = render_markdown(&sample_result());
        assert!(markdown.contains("The important body."));
        assert!(markdown.contains("[Example](https://example.com)"));
        assert!(markdown.contains("Ada (@ada) · 2026-09-03 · 1 image"));
        assert!(markdown.contains("A red rocket"));
        assert!(!markdown.contains("99,999"));
        assert!(!markdown.contains("FxTwitter"));
        assert!(!markdown.contains("operational warning"));
        assert!(!markdown.contains("────"));
    }

    #[test]
    fn markdown_contract_snapshot() {
        assert_eq!(
            render_markdown(&sample_result()),
            "The important body.\n\n### Links\n\n- [Example](https://example.com)\n\n- Media description: A red rocket\n\n_[Ada (@ada) · 2026-09-03 · 1 image](https://x.com/ada/status/123)_"
        );
    }

    #[test]
    fn markdown_includes_best_video_urls_but_not_image_urls() {
        let mut result = sample_result();
        result.post.media.push(Media {
            alt_text: None,
            duration_ms: Some(1_000),
            height: Some(720),
            preview_image_url: Some("https://image.example/video.jpg".to_owned()),
            media_type: Some("video".to_owned()),
            url: None,
            variants: vec![
                crate::MediaVariant {
                    bit_rate: Some(256_000),
                    content_type: Some("video/mp4".to_owned()),
                    url: "https://video.example/480x852/low.mp4".to_owned(),
                },
                crate::MediaVariant {
                    bit_rate: Some(2_000_000),
                    content_type: Some("video/mp4".to_owned()),
                    url: "https://video.example/720x1280/best.mp4".to_owned(),
                },
            ],
            width: Some(1280),
        });

        let markdown = render_markdown(&result);
        assert!(markdown.contains("### Videos"));
        assert!(markdown.contains("https://video.example/720x1280/best.mp4"));
        assert!(!markdown.contains("https://video.example/480x852/low.mp4"));
        assert!(!markdown.contains("https://image.example/1.jpg"));
        assert!(!markdown.contains("https://image.example/video.jpg"));
    }

    #[test]
    fn markdown_preserves_article_images_as_document_content() {
        let mut result = sample_result();
        result.post.links.clear();
        result.post.media.clear();
        result.post.kind = "article".to_owned();
        result.post.article = Some(crate::Article {
            cover_media_url: Some("https://image.example/cover.jpg".to_owned()),
            created_at: None,
            embedded_post_ids: Vec::new(),
            id: Some("article-1".to_owned()),
            links: Vec::new(),
            markdown: "Before\n\n![diagram](<https://image.example/body.jpg>)\n\nAfter".to_owned(),
            media: vec![Media {
                alt_text: Some("diagram".to_owned()),
                duration_ms: None,
                height: None,
                preview_image_url: None,
                media_type: Some("photo".to_owned()),
                url: Some("https://image.example/body.jpg".to_owned()),
                variants: Vec::new(),
                width: None,
            }],
            modified_at: None,
            preview_text: None,
            text: "Before\n\nAfter".to_owned(),
            title: Some("Visual article".to_owned()),
        });

        let markdown = render_markdown(&result);
        assert!(markdown
            .starts_with("# Visual article\n\n![](<https://image.example/cover.jpg>)\n\nBefore"));
        assert!(markdown.contains("![diagram](<https://image.example/body.jpg>)"));
        assert!(!markdown.contains("文章媒体"));
    }

    #[test]
    fn text_contains_only_content() {
        assert_eq!(render_text(&sample_result()), "The important body.");
    }

    #[test]
    fn parses_output_formats_and_aliases() {
        assert_eq!("md".parse(), Ok(OutputFormat::Markdown));
        assert_eq!("txt".parse(), Ok(OutputFormat::Text));
        assert_eq!("json".parse(), Ok(OutputFormat::Json));
        assert_eq!("human".parse(), Ok(OutputFormat::Human));
    }
}
