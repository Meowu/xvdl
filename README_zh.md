# xread（Rust）

[English](README.md) | 简体中文

一个无需 X API Token 的公开 X/Twitter 内容读取器。核心逻辑只实现一次，同时供 Rust 库、原生 CLI 和 Cloudflare Worker 使用。

数据优先来自第三方免费结构化源 [FxTwitter](https://github.com/FixTweet/FxTwitter)；正文读取失败时，会退回 X 的免 Token oEmbed。原 `xread.mjs` 中需要 Bearer Token 的官方 API、`--api` 和 `--archive` 分支已经移除。

## 现在适合做什么

- 直接把默认 Markdown 输出交给 LLM；
- 读取普通推文、长推文、Article、引用、转推以及可选回复；
- 提取图片描述、展开后的外链和视频/GIF；Article 图片按正文位置还原；
- 每个视频选择一个 MP4，支持最佳、最低或目标清晰度；
- 在 CLI 中流式下载全部视频或指定视频；
- 在 Worker 中提供 JSON、Markdown、纯文本和人类可读文本 API。

免费回复源只提供首批精选或最新评论，不能当作完整历史归档。如果发生降级或回复不完整，Markdown 会保留一条有语义的说明；运行诊断则写入 `stderr`，不会污染交给 LLM 的 `stdout`。

## 默认输出为什么是 Markdown

正文读取默认只保留理解内容所需的信息：

- 正文或 Article；
- 作者、日期和原帖链接；
- 引用/转推正文与请求的回复；
- 媒体数量、图片 alt 文本、最佳 MP4 视频地址和正文中没有出现的外链；
- Article 的标题、段落、标题层级、列表、引用、代码、行内样式、链接和正文图片；
- 降级或回复不完整这类会改变语义的提示。

普通推文的图片 URL 默认省略，因为对 LLM 来说通常只有噪声；Article 是一篇完整文档，其中的封面和正文图片会按原位置保留为标准 Markdown 图片。默认还会省略点赞数、浏览数、后端名称、内部 ID、头像、装饰线和所有媒体变体；视频/GIF 会保留每个媒体的一个最佳 MP4 地址。需要完整字段（包括 `article.media`）时使用 JSON，需要终端详情时使用 `human`。

## CLI 快速开始

要求 Rust 1.85 或更新版本：

```bash
cargo install --path .

# 默认输出紧凑 Markdown
xread https://x.com/Interior/status/463440424141459456

# 从 stdin 读取 URL 或包含 URL 的文本
printf '%s\n' 'https://x.com/Interior/status/463440424141459456' | xread -

# 纯正文；适合最小上下文
xread 463440424141459456 --format text

# 完整、稳定的结构化数据；JSON 默认紧凑
xread 463440424141459456 --format json
xread 463440424141459456 --json --pretty

# 回复和对话串
xread 463440424141459456 --replies --limit 50
xread 463440424141459456 --thread --sort recent
```

输出格式：

| 格式 | 用途 | 包含内容 |
|---|---|---|
| `markdown` / `md` | 默认，交给 LLM | 重要正文、出处、媒体摘要、最佳视频地址、语义提示 |
| `text` / `txt` | 最小纯文本 | 正文、引用/转推正文、请求的回复 |
| `json` | 程序消费 | 全部规范化字段；加 `--pretty` 后缩进 |
| `human` | 人工检查 | 指标、媒体 URL、来源等终端详情 |

`--quiet` 只关闭 `stderr` 中的警告和下载进度，不会吞掉正文、URL、路径或错误。

## 视频直链与下载

视频提取会包含已解析的引用/转推内容，因此主推文没有视频、引用推文有视频时也能返回直链。
顺序是每条推文自身的视频在前，再递归收集引用和转推中的视频，并按 URL 去重。
`quality` 对所有视频生效，`video` 按此顺序从 1 编号。
CLI 直链/下载、Worker `/videos` 和旧的 `/<X URL>` 路径共用这套逻辑，旧路径仍返回 URL 的 JSON 数组。

提取直链：

```bash
# 每个视频一个最佳码率 MP4 URL，默认一行一个
xread POST_URL --videos

# 选不超过 720p 的最高一档；竖屏视频按短边计算
xread POST_URL --videos --quality 720

# 最低码率，且只要第二个视频
xread POST_URL --videos --quality worst --video 2

# URL 数组
xread POST_URL --videos --format json --pretty
```

下载到本地：

```bash
# 流式下载所有视频，默认写入当前目录
xread POST_URL --download

# 只下载第一个视频到指定目录
xread POST_URL --download --video 1 --quality 720 --output-dir ./videos

# 只有显式指定时才覆盖同名文件
xread POST_URL --download --output-dir ./videos --force
```

文件名是 `<用户名>-<推文ID>-<序号>.mp4`。下载先写入同目录的私有 `.part` 临时文件，完成后再重命名，因此失败不会留下一个看似完整的目标文件。程序不会默认覆盖已有文件；大文件按块写入，不会整体载入内存。

`--quality 720` 的含义是选择“不高于目标短边的最高一档”；如果所有变体都更高，则选择最小的一档。缺少分辨率信息时退回最高码率。

## CLI 参数

```text
    --format <FORMAT>        markdown、text、json 或 human
-j, --json                   --format json 的兼容简写
    --pretty                 美化 JSON
    --videos                 输出 MP4 直链
    --download               下载 MP4
    --output-dir <DIR>       下载目录，默认当前目录
    --video <INDEX>          只选第几个视频，从 1 开始
    --quality <QUALITY>      best、worst 或 144..4320，如 720
    --force                  覆盖同名下载文件
-q, --quiet                  关闭警告和下载进度
    --replies                直接回复
    --thread                 整段对话中的回复
    --limit <1-1000>         回复上限，默认 20
    --sort <SORT>            relevance（默认）或 recent
    --lang <LANG>            oEmbed 语言，默认 en
    --timeout <MS>           单次请求超时，默认 12000
    --retries <0-5>          重试次数，默认 2
    --community-base <URL>   自托管 FxTwitter v2 根地址
```

也可以通过 `XREAD_COMMUNITY_BASE_URL` 设置自托管源。完整说明见 `xread --help`。

## Rust 库

读取与渲染分开：调用者可以保留强类型数据，也可以选择一种视图。

```rust,no_run
use xvdl::{render_markdown, ReadOptions, ReaderConfig, XReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reader = XReader::new(ReaderConfig::default())?;
    let result = reader
        .read("463440424141459456", &ReadOptions::default())
        .await?;

    println!("{}", render_markdown(&result));
    Ok(())
}
```

选择视频清晰度：

```rust,no_run
# use xvdl::{ReaderConfig, VideoQuality, XReader};
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let reader = XReader::new(ReaderConfig::default())?;
let urls = reader
    .video_urls_with_quality("https://x.com/user/status/123", VideoQuality::Height(720))
    .await?;
# Ok(())
# }
```

原来的 `XReader::video_urls` 仍然存在，等价于 `VideoQuality::Best`。原生构建还公开静默的 `download_videos`，以及可通过 `DownloadEvent` 自己呈现进度的 `download_videos_with_progress`；文件系统相关 API 不会进入 WASM，因为 Worker 没有持久本地文件系统。

## Cloudflare Worker

```bash
worker-build --release
npx wrangler dev
npx wrangler deploy
```

推荐把 URL 放在经过编码的 query 参数中：

```text
GET /
GET /read?url=463440424141459456
GET /read?url=463440424141459456&format=markdown
GET /read?url=463440424141459456&replies=thread&sort=recent&format=text
GET /videos?url=463440424141459456&quality=720&video=1
GET /health
```

直接打开 `/` 会返回类似 `--help` 的紧凑纯文本，其中包含接口、参数和示例。程序需要机器可读的服务说明时，使用 `/?format=json`，或访问没有 `url` 参数的 `/read`。

Worker 的读取接口默认仍返回 JSON；`format=markdown|md` 返回 `text/markdown`，`format=text|txt` 和 `format=human` 返回 `text/plain`。视频接口也支持这四种格式。旧形式 `GET /https://x.com/user/status/123` 继续返回视频 URL JSON 数组。

Worker 参数：

| 参数 | 值 | 默认值 |
|---|---|---|
| `url` | X URL、推文 ID 或含 URL 的文本 | 必填 |
| `replies` | `direct` / `replies` / `thread` | 不读取回复 |
| `limit` | `1..=1000` | `20` |
| `sort` | `relevance` / `recent` | `relevance` |
| `lang` | `en`、`zh-cn` 等 | `en` |
| `format` | `json` / `markdown` / `text` / `human` | `json` |
| `quality` | `best` / `worst` / `144..4320` | `best` |
| `video` | 从 1 开始的视频序号 | 全部 |

Worker 不提供 `/download` 代理：视频文件可能很大，代理会增加执行时长、流量成本和失败面。用 `/videos` 取得直链后由客户端下载更合适。为了避免 SSRF，公开请求也不能覆盖上游地址；部署者可以设置 `XREAD_COMMUNITY_BASE_URL`、`XREAD_TIMEOUT_MS` 和 `XREAD_RETRIES`。

## 代码设计

```text
CLI ─────┐
         ├─> XReader ─> FxTwitter v2 ─> Post / ReadResult ─> Markdown/Text/JSON/Human
Worker ──┘       │
                 └─ 正文读取失败 ─> X oEmbed

CLI download ─> 结构化 Post ─> 选择 MP4 ─> 分块写入 .part ─> 原子重命名
```

职责分层：

- `src/model.rs`：稳定的数据类型、读取选项和视频清晰度策略；
- `src/parse.rs`：安全解析 URL、ID 和混合文本；
- `src/client.rs`：HTTP、超时、重试、回复策略和 oEmbed 降级；
- `src/normalize.rs`：把变化较快的上游 JSON 转成稳定 Rust 类型，并展开短链；
- `src/render.rs`：Markdown、纯文本和人类可读视图；
- `src/download.rs`：仅原生平台使用的流式下载；
- `src/main.rs`：CLI 参数、stdout/stderr 与退出码；
- `src/lib.rs`：公开 API 和仅 WASM 编译的 Worker 适配层。

这样 CLI 与 Worker 不会复制业务逻辑，网络数据的不稳定性也被限制在 `client` / `normalize` 边界内。

## 从代码学习 Rust

建议按以下顺序阅读：

1. `src/model.rs`：用 `struct` / `enum` 表达业务状态；
2. `src/parse.rs`：用 `Option`、`Result` 和 `?` 处理缺失与失败；
3. `src/client.rs`：理解 `async`、借用 `&str` 和共享业务服务；
4. `src/normalize.rs`：只在不稳定 JSON 边界使用 `serde_json::Value`，随后立即转成强类型；
5. `src/render.rs`：同一个模型如何拥有多个输出视图；
6. `src/download.rs`：异步分块读取、文件所有权和失败清理；
7. `src/main.rs` / `src/lib.rs`：比较 CLI 与 Worker 适配层。

几个关键点：

- `&str` 是借用，函数读取字符串但不取得所有权；
- `Result<T, XReadError>` 明确表示成功或可处理错误，`?` 会在失败时提前返回；
- `Option<T>` 明确表示“可能不存在”，而不是拿空字符串充当缺失值；
- `VideoQuality` 用枚举限制合法策略，避免在业务深处传递含义模糊的字符串；
- `#[cfg(target_arch = "wasm32")]` 和相反条件把 Worker、文件系统能力隔离开；
- 下载先写临时文件再改名，是用文件系统操作表达“要么完整成功，要么没有最终文件”。

## 验证

```bash
cargo fmt --all --check
cargo test
cargo clippy --all-targets -- -D warnings
worker-build --release
```

## 许可

MIT
