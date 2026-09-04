//! Token-free X/Twitter reader shared by a native CLI and Cloudflare Workers.
//!
//! The crate is layered intentionally:
//!
//! - [`parse`] accepts user-facing URL forms.
//! - [`client`] owns fetch, retry, fallback, and reply selection policy.
//! - [`model`] is the stable JSON/library contract.
//! - [`render`] is only a human-readable view of that contract.
//!
//! The Worker adapter at the bottom is compiled only for WebAssembly. Native
//! callers therefore use this as a normal Rust library without depending on
//! Worker request/response types.

mod client;
#[cfg(not(target_arch = "wasm32"))]
mod download;
mod error;
mod model;
mod normalize;
mod parse;
mod render;

pub use client::XReader;
#[cfg(not(target_arch = "wasm32"))]
pub use download::{
    download_videos, download_videos_with_progress, DownloadEvent, DownloadOptions, DownloadedVideo,
};
pub use error::{ErrorKind, ErrorResponse, XReadError};
pub use model::{
    Article, Author, Link, Media, MediaVariant, Metrics, Post, PostReference, ReadOptions,
    ReadResult, ReaderConfig, ReplyInfo, ReplyMode, SortMode, VideoQuality,
    DEFAULT_COMMUNITY_BASE_URL, DEFAULT_LIMIT, DEFAULT_RETRIES, DEFAULT_TIMEOUT_MS, MAX_LIMIT,
};
pub use normalize::snowflake_timestamp;
pub use parse::parse_post_reference;
pub use render::{render_human, render_markdown, render_text, OutputFormat};

#[cfg(target_arch = "wasm32")]
mod worker_adapter {
    use std::{str::FromStr, time::Duration};

    use serde::{Deserialize, Serialize};
    use worker::{
        event, Context, Env, Headers, Method, Request, Response, ResponseBuilder, Result,
    };

    use crate::{
        render_human, render_markdown, render_text, ErrorResponse, OutputFormat, ReadOptions,
        ReaderConfig, ReplyMode, SortMode, VideoQuality, XReadError, XReader, DEFAULT_RETRIES,
        DEFAULT_TIMEOUT_MS,
    };

    #[derive(Debug, Default, Deserialize)]
    struct Query {
        url: Option<String>,
        replies: Option<String>,
        limit: Option<usize>,
        sort: Option<String>,
        lang: Option<String>,
        format: Option<String>,
        quality: Option<String>,
        video: Option<usize>,
    }

    #[derive(Debug, Serialize)]
    struct Usage<'a> {
        service: &'a str,
        usage: &'a str,
        video_usage: &'a str,
        example: &'a str,
        formats: [&'a str; 4],
        reply_modes: [&'a str; 2],
    }

    #[derive(Debug, Clone, Copy)]
    enum Operation {
        Read,
        Videos,
    }

    #[event(fetch)]
    pub async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
        console_error_panic_hook::set_once();

        if req.method() == Method::Options {
            return Ok(ResponseBuilder::new()
                .with_headers(cors_headers()?)
                .with_status(204)
                .empty());
        }
        if req.method() != Method::Get {
            return error_response(&XReadError::invalid_input("只支持 GET 请求。"), Some(405));
        }

        let path = req.path();
        if path == "/health" {
            return ResponseBuilder::new()
                .with_headers(cors_headers()?)
                .from_json(&serde_json::json!({ "ok": true }));
        }
        if path == "/download" {
            return error_response(
                &XReadError::invalid_input(
                    "Worker 不代理视频文件；请使用 /videos 获取直链，或用 CLI 的 --download。",
                ),
                None,
            );
        }

        let query = match req.query::<Query>() {
            Ok(query) => query,
            Err(error) => {
                return error_response(
                    &XReadError::invalid_input(format!("无法解析查询参数：{error}")),
                    None,
                )
            }
        };
        let operation = if path == "/videos" || !matches!(path.as_str(), "/" | "/read") {
            Operation::Videos
        } else {
            Operation::Read
        };
        let input = query.url.clone().or_else(|| legacy_path_input(&path));
        let Some(input) = input else {
            let usage = Usage {
                service: "xread",
                usage: "GET /read?url=<X URL or status ID>",
                video_usage: "GET /videos?url=<X URL or status ID>",
                example: "/read?url=https%3A%2F%2Fx.com%2FInterior%2Fstatus%2F463440424141459456&replies=direct",
                formats: ["json", "markdown", "text", "human"],
                reply_modes: ["direct", "thread"],
            };
            return ResponseBuilder::new()
                .with_headers(cors_headers()?)
                .from_json(&usage);
        };

        let config = match worker_config(&env) {
            Ok(config) => config,
            Err(error) => return error_response(&error, None),
        };
        let reader = match XReader::new(config) {
            Ok(reader) => reader,
            Err(error) => {
                let error = XReadError::internal(format!("Worker 配置无效：{}", error.message));
                return error_response(&error, None);
            }
        };

        let format = match worker_format(&query) {
            Ok(format) => format,
            Err(error) => return error_response(&error, None),
        };

        match operation {
            Operation::Videos => {
                let quality = match query
                    .quality
                    .as_deref()
                    .map(VideoQuality::from_str)
                    .transpose()
                {
                    Ok(quality) => quality.unwrap_or_default(),
                    Err(error) => return error_response(&error, None),
                };
                match reader.video_urls_with_quality(&input, quality).await {
                    Ok(urls) => match select_video_urls(urls, query.video) {
                        Ok(urls) => render_video_response(&urls, query.video, format),
                        Err(error) => error_response(&error, None),
                    },
                    Err(error) => error_response(&error, None),
                }
            }
            Operation::Read => {
                if query.quality.is_some() || query.video.is_some() {
                    return error_response(
                        &XReadError::invalid_input(
                            "quality 和 video 只适用于 /videos；CLI 下载请使用 --download。",
                        ),
                        None,
                    );
                }
                let options = match read_options(&query) {
                    Ok(options) => options,
                    Err(error) => return error_response(&error, None),
                };
                match reader.read(&input, &options).await {
                    Ok(result) => match format {
                        OutputFormat::Json => ResponseBuilder::new()
                            .with_headers(cors_headers()?)
                            .from_json(&result),
                        OutputFormat::Markdown => {
                            body_response(render_markdown(&result), "text/markdown; charset=utf-8")
                        }
                        OutputFormat::Text => {
                            body_response(render_text(&result), "text/plain; charset=utf-8")
                        }
                        OutputFormat::Human => {
                            body_response(render_human(&result), "text/plain; charset=utf-8")
                        }
                    },
                    Err(error) => error_response(&error, None),
                }
            }
        }
    }

    fn worker_format(query: &Query) -> std::result::Result<OutputFormat, XReadError> {
        // Keep JSON as the HTTP default for backward compatibility. The CLI's
        // default is Markdown because its most common consumer is now an LLM.
        query
            .format
            .as_deref()
            .unwrap_or("json")
            .parse::<OutputFormat>()
    }

    fn select_video_urls(
        urls: Vec<String>,
        requested: Option<usize>,
    ) -> std::result::Result<Vec<String>, XReadError> {
        let Some(index) = requested else {
            return Ok(urls);
        };
        if index == 0 {
            return Err(XReadError::invalid_input(
                "video 从 1 开始计数，例如 video=1。",
            ));
        }
        urls.get(index - 1)
            .cloned()
            .map(|url| vec![url])
            .ok_or_else(|| {
                XReadError::invalid_input(format!(
                    "video={index} 超出范围；这条推文共有 {} 个视频。",
                    urls.len()
                ))
            })
    }

    fn render_video_response(
        urls: &[String],
        requested: Option<usize>,
        format: OutputFormat,
    ) -> Result<Response> {
        match format {
            OutputFormat::Json => {
                let urls = urls.to_vec();
                ResponseBuilder::new()
                    .with_headers(cors_headers()?)
                    .from_json(&urls)
            }
            OutputFormat::Markdown => {
                let markdown = urls
                    .iter()
                    .enumerate()
                    .map(|(index, url)| {
                        format!("- [Video {}]({url})", requested.unwrap_or(index + 1))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                body_response(markdown, "text/markdown; charset=utf-8")
            }
            OutputFormat::Text | OutputFormat::Human => {
                body_response(urls.join("\n"), "text/plain; charset=utf-8")
            }
        }
    }

    fn body_response(body: String, content_type: &str) -> Result<Response> {
        let headers = cors_headers()?;
        headers.set("content-type", content_type)?;
        // `ResponseBuilder::ok` always rewrites Content-Type to text/plain.
        // `fixed` keeps the explicit Markdown type set above.
        Ok(ResponseBuilder::new()
            .with_headers(headers)
            .fixed(body.into_bytes()))
    }

    fn read_options(query: &Query) -> std::result::Result<ReadOptions, XReadError> {
        let reply_mode = query
            .replies
            .as_deref()
            .map(ReplyMode::from_str)
            .transpose()?;
        let sort = query
            .sort
            .as_deref()
            .map(SortMode::from_str)
            .transpose()?
            .unwrap_or_default();
        let options = ReadOptions {
            reply_mode,
            limit: query.limit.unwrap_or(crate::DEFAULT_LIMIT),
            sort,
            lang: query.lang.as_deref().unwrap_or("en").to_ascii_lowercase(),
        };
        options.validate()?;
        Ok(options)
    }

    fn worker_config(env: &Env) -> std::result::Result<ReaderConfig, XReadError> {
        let mut config = ReaderConfig::default();

        // Only deployment bindings may change the upstream base URL. Accepting
        // it from a public query parameter would turn this Worker into an SSRF proxy.
        if let Ok(value) = env.var("XREAD_COMMUNITY_BASE_URL") {
            config.community_base_url = value.to_string();
        }
        config.timeout = Duration::from_millis(parse_env_number(
            env,
            "XREAD_TIMEOUT_MS",
            DEFAULT_TIMEOUT_MS,
        )?);
        config.retries = parse_env_number(env, "XREAD_RETRIES", u64::from(DEFAULT_RETRIES))?
            .try_into()
            .map_err(|_| XReadError::internal("XREAD_RETRIES 超出 u8 范围。"))?;
        config.validate().map_err(|error| {
            XReadError::internal(format!("Worker 环境变量配置无效：{}", error.message))
        })?;
        Ok(config)
    }

    fn parse_env_number(
        env: &Env,
        name: &str,
        default: u64,
    ) -> std::result::Result<u64, XReadError> {
        let Ok(value) = env.var(name) else {
            return Ok(default);
        };
        value
            .to_string()
            .parse()
            .map_err(|_| XReadError::internal(format!("Worker 环境变量 {name} 必须是非负整数。")))
    }

    fn legacy_path_input(path: &str) -> Option<String> {
        let input = path.trim_start_matches('/');
        if input.is_empty() || matches!(input, "read" | "videos") {
            None
        } else {
            Some(input.to_owned())
        }
    }

    fn error_response(error: &XReadError, status: Option<u16>) -> Result<Response> {
        ResponseBuilder::new()
            .with_headers(cors_headers()?)
            .with_status(status.unwrap_or_else(|| error.response_status()))
            .from_json(&ErrorResponse::from(error))
    }

    fn cors_headers() -> Result<Headers> {
        let headers = Headers::new();
        headers.set("access-control-allow-origin", "*")?;
        headers.set("access-control-allow-methods", "GET, OPTIONS")?;
        headers.set("access-control-allow-headers", "Content-Type")?;
        Ok(headers)
    }
}
