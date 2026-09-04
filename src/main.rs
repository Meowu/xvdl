use std::{
    env,
    io::{self, IsTerminal, Read},
    path::PathBuf,
    process::ExitCode,
    str::FromStr,
    time::Duration,
};

use clap::{ArgAction, Parser};
use xvdl::{
    download_videos_with_progress, render_human, render_markdown, render_text, DownloadEvent,
    DownloadOptions, OutputFormat, ReadOptions, ReaderConfig, ReplyMode, SortMode, VideoQuality,
    XReadError, XReader, DEFAULT_LIMIT, DEFAULT_RETRIES, DEFAULT_TIMEOUT_MS,
};

#[derive(Debug, Parser)]
#[command(
    name = "xread",
    version,
    about = "读取公开 X 推文、回复和视频（无需 X API Token）",
    after_help = "示例：\n  xread https://x.com/Interior/status/463440424141459456\n  xread - < url.txt\n  xread 463440424141459456 --format json --pretty\n  xread https://x.com/user/status/123 --replies --limit 50\n  xread https://x.com/user/status/123 --videos --quality 720\n  xread https://x.com/user/status/123 --download --video 1 --output-dir ./videos"
)]
struct Cli {
    /// X/Twitter 链接、包含链接的文本、纯推文 ID，或 -（从 stdin 读取）
    input: String,

    /// 输出格式：markdown、text、json 或 human；正文默认 markdown
    #[arg(long, value_name = "FORMAT")]
    format: Option<String>,

    /// --format json 的兼容简写
    #[arg(short = 'j', long, conflicts_with = "format", action = ArgAction::SetTrue)]
    json: bool,

    /// 让 JSON 带缩进；默认 JSON 为适合管道传输的紧凑格式
    #[arg(long, action = ArgAction::SetTrue)]
    pretty: bool,

    /// 只输出每个视频所选清晰度的 MP4 直链
    #[arg(long, conflicts_with_all = ["replies", "thread"], action = ArgAction::SetTrue)]
    videos: bool,

    /// 将视频流式下载到本地；Worker 端只提供直链，不代理下载
    #[arg(long, conflicts_with_all = ["videos", "replies", "thread"], action = ArgAction::SetTrue)]
    download: bool,

    /// 下载目录，默认当前目录
    #[arg(long, value_name = "DIR")]
    output_dir: Option<PathBuf>,

    /// 只选择第几个视频（从 1 开始，配合 --videos 或 --download）
    #[arg(long, value_name = "INDEX")]
    video: Option<usize>,

    /// 视频清晰度：best（默认）、worst 或短边像素（如 720）
    #[arg(long)]
    quality: Option<String>,

    /// 覆盖同名下载文件
    #[arg(long, requires = "download", action = ArgAction::SetTrue)]
    force: bool,

    /// 不在 stderr 输出警告和下载进度
    #[arg(short, long, action = ArgAction::SetTrue)]
    quiet: bool,

    /// 读取目标推文的直接回复
    #[arg(long, conflicts_with = "thread", action = ArgAction::SetTrue)]
    replies: bool,

    /// 读取目标所在整段对话的回复
    #[arg(long, conflicts_with = "replies", action = ArgAction::SetTrue)]
    thread: bool,

    /// 最多返回多少条回复
    #[arg(long, default_value_t = DEFAULT_LIMIT)]
    limit: usize,

    /// 回复排序：relevance 或 recent
    #[arg(long, default_value = "relevance")]
    sort: String,

    /// oEmbed 语言代码
    #[arg(long, default_value = "en")]
    lang: String,

    /// 单次上游请求超时（毫秒）
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    timeout: u64,

    /// 可重试错误的重试次数
    #[arg(long, default_value_t = DEFAULT_RETRIES)]
    retries: u8,

    /// FxTwitter v2 API 根地址（也可设置 XREAD_COMMUNITY_BASE_URL）
    #[arg(long)]
    community_base: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{}", error.message);
            if let Some(hint) = error.hint {
                eprintln!("提示：{hint}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), XReadError> {
    if cli.video.is_some() && !cli.videos && !cli.download {
        return Err(XReadError::invalid_input(
            "--video 需要和 --videos 或 --download 一起使用。",
        ));
    }
    if cli.output_dir.is_some() && !cli.download {
        return Err(XReadError::invalid_input(
            "--output-dir 需要和 --download 一起使用。",
        ));
    }
    let input = read_input(&cli.input)?;
    let media_action = cli.videos || cli.download;
    if cli.quality.is_some() && !media_action {
        return Err(XReadError::invalid_input(
            "--quality 需要和 --videos 或 --download 一起使用。",
        ));
    }
    let default_format = if media_action {
        OutputFormat::Text
    } else {
        OutputFormat::Markdown
    };
    let format = resolve_format(cli.format.as_deref(), cli.json, default_format)?;
    if cli.pretty && format != OutputFormat::Json {
        return Err(XReadError::invalid_input(
            "--pretty 只对 --format json 或 --json 生效。",
        ));
    }
    let quality = VideoQuality::from_str(cli.quality.as_deref().unwrap_or("best"))?;

    let mut config = ReaderConfig {
        timeout: Duration::from_millis(cli.timeout),
        retries: cli.retries,
        ..ReaderConfig::default()
    };
    if let Some(base_url) = cli
        .community_base
        .or_else(|| env::var("XREAD_COMMUNITY_BASE_URL").ok())
    {
        config.community_base_url = base_url;
    }

    let reader = XReader::new(config)?;
    if cli.download {
        let quiet = cli.quiet;
        let stderr_is_terminal = io::stderr().is_terminal();
        let downloaded = download_videos_with_progress(
            &reader,
            &input,
            &DownloadOptions {
                output_dir: cli.output_dir.unwrap_or_else(|| PathBuf::from(".")),
                video: cli.video,
                quality,
                force: cli.force,
            },
            |event| {
                if !quiet {
                    report_download(event, stderr_is_terminal);
                }
            },
        )
        .await?;
        print_downloads(&downloaded, format, cli.pretty)?;
        return Ok(());
    }
    if cli.videos {
        let urls = reader.video_urls_with_quality(&input, quality).await?;
        let urls = select_video_urls(urls, cli.video)?;
        print_video_urls(&urls, cli.video, format, cli.pretty)?;
        return Ok(());
    }

    let reply_mode = if cli.replies {
        Some(ReplyMode::Direct)
    } else if cli.thread {
        Some(ReplyMode::Thread)
    } else {
        None
    };
    let options = ReadOptions {
        reply_mode,
        limit: cli.limit,
        sort: SortMode::from_str(&cli.sort)?,
        lang: cli.lang.to_ascii_lowercase(),
    };
    let result = reader.read(&input, &options).await?;
    if !cli.quiet {
        for warning in &result.warnings {
            eprintln!("警告：{warning}");
        }
    }
    match format {
        OutputFormat::Markdown => println!("{}", render_markdown(&result)),
        OutputFormat::Text => println!("{}", render_text(&result)),
        OutputFormat::Human => println!("{}", render_human(&result)),
        OutputFormat::Json => print_json(&result, cli.pretty)?,
    }
    Ok(())
}

fn read_input(value: &str) -> Result<String, XReadError> {
    let input = if value == "-" {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|error| XReadError::invalid_input(format!("无法读取 stdin：{error}")))?;
        buffer
    } else {
        value.to_owned()
    };
    let input = input.trim();
    if input.is_empty() {
        Err(XReadError::invalid_input("输入不能为空。"))
    } else {
        Ok(input.to_owned())
    }
}

fn resolve_format(
    format: Option<&str>,
    json_alias: bool,
    default: OutputFormat,
) -> Result<OutputFormat, XReadError> {
    if json_alias {
        Ok(OutputFormat::Json)
    } else {
        format
            .map(OutputFormat::from_str)
            .transpose()
            .map(|value| value.unwrap_or(default))
    }
}

fn select_video_urls(
    urls: Vec<String>,
    requested: Option<usize>,
) -> Result<Vec<String>, XReadError> {
    let Some(index) = requested else {
        return Ok(urls);
    };
    if index == 0 {
        return Err(XReadError::invalid_input(
            "video 从 1 开始计数，例如 --video 1。",
        ));
    }
    urls.get(index - 1)
        .cloned()
        .map(|url| vec![url])
        .ok_or_else(|| {
            XReadError::invalid_input(format!(
                "--video {index} 超出范围；这条推文共有 {} 个视频。",
                urls.len()
            ))
        })
}

fn print_video_urls(
    urls: &[String],
    requested: Option<usize>,
    format: OutputFormat,
    pretty: bool,
) -> Result<(), XReadError> {
    match format {
        OutputFormat::Json => print_json(urls, pretty),
        OutputFormat::Markdown => {
            for (index, url) in urls.iter().enumerate() {
                println!("- [Video {}]({url})", requested.unwrap_or(index + 1));
            }
            Ok(())
        }
        OutputFormat::Text | OutputFormat::Human => {
            println!("{}", urls.join("\n"));
            Ok(())
        }
    }
}

fn print_downloads(
    downloaded: &[xvdl::DownloadedVideo],
    format: OutputFormat,
    pretty: bool,
) -> Result<(), XReadError> {
    match format {
        OutputFormat::Json => print_json(downloaded, pretty),
        OutputFormat::Markdown => {
            for video in downloaded {
                println!("- [Video {}](<{}>)", video.index, video.path.display());
            }
            Ok(())
        }
        OutputFormat::Text | OutputFormat::Human => {
            for video in downloaded {
                println!("{}", video.path.display());
            }
            Ok(())
        }
    }
}

fn report_download(event: DownloadEvent, terminal: bool) {
    match event {
        DownloadEvent::Started { index, path, .. } => {
            eprintln!("下载视频 {index} → {}", path.display());
        }
        DownloadEvent::Progress {
            index,
            downloaded_bytes,
            total_bytes,
        } if terminal => {
            if let Some(total) = total_bytes.filter(|total| *total > 0) {
                eprint!(
                    "\r下载视频 {index}：{:>3}%",
                    downloaded_bytes.saturating_mul(100) / total
                );
            } else {
                eprint!("\r下载视频 {index}：{downloaded_bytes} bytes");
            }
        }
        DownloadEvent::Finished { index, bytes, .. } if terminal => {
            eprintln!("\r下载视频 {index}：完成（{bytes} bytes）");
        }
        DownloadEvent::Finished { index, bytes, .. } => {
            eprintln!("视频 {index} 下载完成（{bytes} bytes）");
        }
        DownloadEvent::Progress { .. } => {}
    }
}

fn print_json<T: serde::Serialize + ?Sized>(value: &T, pretty: bool) -> Result<(), XReadError> {
    let json = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(|error| XReadError::internal(format!("无法生成 JSON：{error}")))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_one_video_by_one_based_index() {
        let urls = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(
            select_video_urls(urls.clone(), Some(2)).unwrap(),
            vec!["two"]
        );
        assert!(select_video_urls(urls.clone(), Some(0)).is_err());
        assert!(select_video_urls(urls, Some(3)).is_err());
    }

    #[test]
    fn default_format_depends_on_operation() {
        assert_eq!(
            resolve_format(None, false, OutputFormat::Markdown).unwrap(),
            OutputFormat::Markdown
        );
        assert_eq!(
            resolve_format(None, true, OutputFormat::Markdown).unwrap(),
            OutputFormat::Json
        );
    }
}
