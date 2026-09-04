# xread (Rust)

English | [简体中文](README_zh.md)

A token-free reader for public X/Twitter content. One Rust core powers a reusable library, a native CLI, and a Cloudflare Worker.

Structured data comes from the free third-party [FxTwitter](https://github.com/FixTweet/FxTwitter) service. Text reads fall back to X's token-free oEmbed endpoint. The Bearer-token API, `--api`, and `--archive` paths from `xread.mjs` are intentionally omitted.

## Features

- LLM-oriented, compact Markdown by default in the CLI
- Normal posts, long posts, Articles, quotes, reposts, and optional replies
- Expanded links, media descriptions, and normalized structured data
- Article headings, lists, quotes, inline styles, links, and images in document order
- One MP4 per video/GIF with `best`, `worst`, or target-resolution selection
- Native streaming downloads for all videos or one selected video
- JSON, Markdown, plain-text, and detailed human views
- Shared timeout, retry, fallback, and validation behavior across adapters

Free replies contain only the first ranked/recent page and are not a complete archive. Diagnostics go to `stderr`; semantic limitations remain in Markdown when they affect how the content should be interpreted.

The compact view omits image URLs from ordinary posts, where they are usually noise for an LLM. Article cover and body images are different: they are part of the document and remain as standard Markdown images at their original positions. JSON also exposes normalized Article media in `article.media`.

## CLI

Rust 1.85 or newer is required.

```bash
cargo install --path .

# Compact Markdown by default
xread https://x.com/Interior/status/463440424141459456

# Read a URL or text from stdin
printf '%s\n' 'https://x.com/Interior/status/463440424141459456' | xread -

# Other content views
xread 463440424141459456 --format text
xread 463440424141459456 --format json
xread 463440424141459456 --json --pretty
xread 463440424141459456 --format human

# Replies
xread 463440424141459456 --replies --limit 50
xread 463440424141459456 --thread --sort recent
```

Formats:

| Format | Intended use |
|---|---|
| `markdown` / `md` | Default LLM input: content, provenance, media summary, best video URLs, semantic notes |
| `text` / `txt` | Minimal body, quote/repost content, and requested replies |
| `json` | Complete normalized contract; compact unless `--pretty` is used |
| `human` | Detailed terminal view including metrics, media URLs, and source |

`--quiet` suppresses warnings and download progress on `stderr`, but never content, selected URLs, downloaded paths, or errors.

## Video URLs and downloads

```bash
# One best-bitrate MP4 per video, one URL per line
xread POST_URL --videos

# Highest representation no larger than a 720-pixel short edge
xread POST_URL --videos --quality 720

# Only the second video at the lowest available bitrate
xread POST_URL --videos --quality worst --video 2

# Stream all videos to disk
xread POST_URL --download --output-dir ./videos

# Download one representation; existing files require explicit replacement
xread POST_URL --download --video 1 --quality 720 --output-dir ./videos --force
```

Files are named `<username>-<post-id>-<index>.mp4`. Downloads are streamed into a private `.part` file and renamed only after completion, so large files are not buffered in memory and interrupted downloads do not look complete.

Run `xread --help` for all options. Use `--community-base` or `XREAD_COMMUNITY_BASE_URL` for a self-hosted FxTwitter v2 instance.

## Rust library

```rust,no_run
use xvdl::{render_markdown, ReadOptions, ReaderConfig, VideoQuality, XReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reader = XReader::new(ReaderConfig::default())?;
    let result = reader
        .read("463440424141459456", &ReadOptions::default())
        .await?;
    println!("{}", render_markdown(&result));

    let videos = reader
        .video_urls_with_quality("463440424141459456", VideoQuality::Height(720))
        .await?;
    println!("{videos:?}");
    Ok(())
}
```

`XReader::video_urls` remains the shorthand for `VideoQuality::Best`. Native builds expose a silent `download_videos` plus `download_videos_with_progress` for callers that want to render `DownloadEvent`s themselves. Filesystem APIs are excluded from WASM builds.

## Cloudflare Worker

```bash
worker-build --release
npx wrangler dev
npx wrangler deploy
```

Use an encoded query parameter when passing a full URL:

```text
GET /
GET /read?url=463440424141459456
GET /read?url=463440424141459456&format=markdown
GET /read?url=463440424141459456&replies=thread&sort=recent&format=text
GET /videos?url=463440424141459456&quality=720&video=1
GET /health
```

Opening `/` returns compact, plain-text `--help` output with endpoints, options, and examples. Machine-readable service discovery remains available at `/?format=json` and from `/read` without a URL.

The Worker keeps JSON as its default for API reads. `format=markdown|md` returns `text/markdown`; `format=text|txt` and `format=human` return `text/plain`. Video routes support the same formats. The legacy `GET /https://x.com/user/status/123` form still returns a JSON URL array.

Supported query parameters are:

- reading: `url`, `replies=direct|replies|thread`, `limit=1..1000`, `sort=relevance|recent`, and `lang`;
- rendering: `format=json|markdown|text|human`;
- video selection: `quality=best|worst|144..4320` and one-based `video`.

The Worker does not proxy `/download`: large media transfer is better performed by the client after calling `/videos`. Public callers cannot override the upstream base URL, which avoids turning the Worker into an SSRF proxy. Deployers may configure `XREAD_COMMUNITY_BASE_URL`, `XREAD_TIMEOUT_MS`, and `XREAD_RETRIES`.

## Architecture

```text
CLI ─────┐
         ├─> XReader ─> FxTwitter v2 ─> Post / ReadResult ─> Markdown/Text/JSON/Human
Worker ──┘       │
                 └─ text fallback ─> X oEmbed

CLI download ─> structured Post ─> select MP4 ─> stream to .part ─> rename
```

The focused modules are `model` (stable types and policies), `parse` (safe input parsing), `client` (HTTP and fallback), `normalize` (loose upstream JSON to typed data), `render` (output views), and native-only `download`. `main.rs` and the Worker adapter in `lib.rs` remain thin protocol layers.

## Verification

```bash
cargo fmt --all --check
cargo test
cargo clippy --all-targets -- -D warnings
worker-build --release
```

## License

MIT
