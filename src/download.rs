//! Native-only, streaming video downloads.
//!
//! A Cloudflare Worker has no durable local filesystem, so downloading is a
//! CLI/library feature rather than part of the Worker adapter. The Worker can
//! still return the selected MP4 URLs through `/videos`.

use std::{
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::{parse_post_reference, VideoQuality, XReadError, XReader};

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Directory in which the final `.mp4` files are created.
    pub output_dir: PathBuf,
    /// Download one video by its one-based position, or all videos when absent.
    pub video: Option<usize>,
    pub quality: VideoQuality,
    /// Replace an existing final file with the same deterministic name.
    pub force: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("."),
            video: None,
            quality: VideoQuality::Best,
            force: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadedVideo {
    /// One-based position of this video in the post.
    pub index: usize,
    pub source_url: String,
    pub path: PathBuf,
    pub bytes: u64,
}

/// Events let a CLI display progress without making the reusable library write
/// to a terminal. A GUI could consume the same events differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    Started {
        index: usize,
        path: PathBuf,
        total_bytes: Option<u64>,
    },
    Progress {
        index: usize,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Finished {
        index: usize,
        path: PathBuf,
        bytes: u64,
    },
}

/// Fetch a post once, choose one MP4 representation per video, and stream the
/// selected files to disk. Streaming keeps large videos out of memory.
pub async fn download_videos(
    reader: &XReader,
    input: &str,
    options: &DownloadOptions,
) -> Result<Vec<DownloadedVideo>, XReadError> {
    download_videos_with_progress(reader, input, options, |_| {}).await
}

/// The callback is synchronous and should return quickly; it is intended for
/// progress display, not for performing another download or blocking I/O.
pub async fn download_videos_with_progress<F>(
    reader: &XReader,
    input: &str,
    options: &DownloadOptions,
    mut report: F,
) -> Result<Vec<DownloadedVideo>, XReadError>
where
    F: FnMut(DownloadEvent),
{
    if options.video == Some(0) {
        return Err(XReadError::invalid_input(
            "video 从 1 开始计数，例如 --video 1。",
        ));
    }

    let reference = parse_post_reference(input)?;
    let post = reader.media_post(input).await?;
    let urls = post.video_urls_with_quality(options.quality);
    if urls.is_empty() {
        return Err(XReadError::no_video("这条推文没有可下载的 MP4 视频。"));
    }
    let selected = select_videos(&urls, options.video)?;

    tokio::fs::create_dir_all(&options.output_dir)
        .await
        .map_err(|error| {
            XReadError::internal(format!(
                "无法创建输出目录 {}：{error}",
                options.output_dir.display()
            ))
        })?;

    let username = post
        .author
        .username
        .as_deref()
        .or(reference.username.as_deref())
        .unwrap_or("x");
    let planned = selected
        .into_iter()
        .map(|(index, url)| {
            let file_name = download_file_name(username, &reference.id, index);
            (index, url, options.output_dir.join(file_name))
        })
        .collect::<Vec<_>>();

    // Validate every destination before downloading the first byte. This
    // avoids saving video 1 before discovering that video 2 already exists.
    if !options.force {
        for (_, _, path) in &planned {
            if tokio::fs::try_exists(path).await.map_err(|error| {
                XReadError::internal(format!("无法检查 {}：{error}", path.display()))
            })? {
                return Err(XReadError::invalid_input(format!(
                    "文件已存在：{}。如需覆盖，请加 --force。",
                    path.display()
                )));
            }
        }
    }

    let client = reqwest::Client::builder()
        .user_agent("xvdl/xread")
        .build()
        .map_err(|error| XReadError::internal(format!("无法创建下载客户端：{error}")))?;

    let mut downloaded = Vec::with_capacity(planned.len());
    for (index, url, path) in planned {
        downloaded
            .push(download_one(&client, index, url, &path, options.force, &mut report).await?);
    }
    Ok(downloaded)
}

fn select_videos(
    urls: &[String],
    requested: Option<usize>,
) -> Result<Vec<(usize, &str)>, XReadError> {
    if let Some(index) = requested {
        if index == 0 {
            return Err(XReadError::invalid_input(
                "video 从 1 开始计数，例如 --video 1。",
            ));
        }
        let Some(url) = urls.get(index.saturating_sub(1)) else {
            return Err(XReadError::invalid_input(format!(
                "--video {index} 超出范围；这条推文共有 {} 个视频。",
                urls.len()
            )));
        };
        return Ok(vec![(index, url.as_str())]);
    }
    Ok(urls
        .iter()
        .enumerate()
        .map(|(index, url)| (index + 1, url.as_str()))
        .collect())
}

async fn download_one<F>(
    client: &reqwest::Client,
    index: usize,
    url: &str,
    destination: &Path,
    force: bool,
    report: &mut F,
) -> Result<DownloadedVideo, XReadError>
where
    F: FnMut(DownloadEvent),
{
    let temporary = temporary_path(destination);
    let attempt = async {
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|error| XReadError::upstream(format!("视频下载请求失败：{error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(
                XReadError::upstream(format!("视频服务器返回 HTTP {}。", status.as_u16()))
                    .with_upstream_status(status.as_u16()),
            );
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                let value = value.to_ascii_lowercase();
                value.starts_with("text/") || value.contains("json")
            })
        {
            return Err(XReadError::invalid_response(
                "视频地址返回了文本而不是媒体文件。",
            ));
        }

        let total = response.content_length();
        report(DownloadEvent::Started {
            index,
            path: destination.to_path_buf(),
            total_bytes: total,
        });
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await
            .map_err(|error| {
                XReadError::internal(format!("无法创建临时文件 {}：{error}", temporary.display()))
            })?;
        let mut written = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| XReadError::upstream(format!("视频下载中断：{error}")))?
        {
            file.write_all(&chunk).await.map_err(|error| {
                XReadError::internal(format!("无法写入 {}：{error}", temporary.display()))
            })?;
            written += chunk.len() as u64;
            report(DownloadEvent::Progress {
                index,
                downloaded_bytes: written,
                total_bytes: total,
            });
        }
        if let Some(expected) = total {
            if expected != written {
                return Err(XReadError::upstream(format!(
                    "视频下载不完整：预期 {expected} 字节，实际收到 {written} 字节。"
                )));
            }
        }
        file.flush().await.map_err(|error| {
            XReadError::internal(format!("无法刷新 {}：{error}", temporary.display()))
        })?;
        drop(file);

        // Delay replacing the old file until the new download is complete.
        if force && tokio::fs::try_exists(destination).await.unwrap_or(false) {
            tokio::fs::remove_file(destination).await.map_err(|error| {
                XReadError::internal(format!("无法覆盖 {}：{error}", destination.display()))
            })?;
        }
        tokio::fs::rename(&temporary, destination)
            .await
            .map_err(|error| {
                XReadError::internal(format!(
                    "无法将下载结果保存为 {}：{error}",
                    destination.display()
                ))
            })?;
        report(DownloadEvent::Finished {
            index,
            path: destination.to_path_buf(),
            bytes: written,
        });
        Ok(DownloadedVideo {
            index,
            source_url: url.to_owned(),
            path: destination.to_path_buf(),
            bytes: written,
        })
    }
    .await;

    if attempt.is_err() {
        // This path is private to the in-progress operation; deleting it never
        // touches a successfully downloaded user file.
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    attempt
}

fn download_file_name(username: &str, post_id: &str, index: usize) -> String {
    format!("{}-{post_id}-{index}.mp4", safe_component(username))
}

fn safe_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "x".to_owned()
    } else {
        sanitized.to_owned()
    }
}

fn temporary_path(destination: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("video.mp4");
    destination.with_file_name(format!(".{file_name}.{}.{}.part", process::id(), nonce))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_one_based_video_or_all() {
        let urls = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(
            select_videos(&urls, None).unwrap(),
            vec![(1, "one"), (2, "two")]
        );
        assert_eq!(select_videos(&urls, Some(2)).unwrap(), vec![(2, "two")]);
        assert!(select_videos(&urls, Some(3)).is_err());
        assert!(select_videos(&urls, Some(0)).is_err());
    }

    #[test]
    fn creates_predictable_safe_file_names() {
        assert_eq!(
            download_file_name("rust_lang", "123", 2),
            "rust_lang-123-2.mp4"
        );
        assert_eq!(download_file_name("../中文", "123", 1), "x-123-1.mp4");
    }
}
