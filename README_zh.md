# XVDL - X/Twitter 视频下载器

[English](README.md) | [简体中文](README_zh.md)

一个基于 Rust 的 Cloudflare Workers 服务，用于生成 X.com（原 Twitter）视频的直接下载链接。

## 功能特点

- 生成 X.com/Twitter 视频的直接下载链接
- 使用 Rust 和 Cloudflare Workers 实现快速高效处理
- 简单的 REST API，易于集成
- 通过 Cloudflare Workers 实现无服务器部署
- 同时支持 x.com 和 twitter.com URL

## 安装

### 前提条件

- [Rust](https://www.rust-lang.org/tools/install) 和 Cargo
- [Node.js](https://nodejs.org/) 和 npm（用于 Cloudflare Wrangler）
- [Wrangler CLI](https://developers.cloudflare.com/workers/wrangler/install-and-update/)（Cloudflare Workers 命令行工具）

### 设置

1. 克隆仓库：
   ```bash
   git clone https://github.com/yourusername/xvdl.git
   cd xvdl
   ```

2. 安装依赖：
   ```bash
   cargo build
   ```

3. 安装 worker-build 工具：
   ```bash
   cargo install worker-build
   ```

## 使用方法

向您部署的 Cloudflare Worker 发送带有 X/Twitter URL 路径的 HTTP 请求：

```
https://your-worker-subdomain.workers.dev/https://x.com/username/status/1234567890
```

如果成功，服务将返回视频的直接下载 URL。

### 示例

请求：
```
GET https://your-worker-subdomain.workers.dev/https://x.com/username/status/1234567890
```

成功响应：
```
https://video-host.com/path/to/video.mp4
```

错误响应（400 Bad Request）：
```
Invalid X URL. Only x.com and twitter.com URLs are supported.
```

错误响应（500 Internal Server Error）：
```
Download job failed
```

## 部署

要将服务部署到 Cloudflare Workers：

1. 使用 Wrangler 登录 Cloudflare：

   ```bash
   wrangler login
   ```

2. 构建并部署服务：

   ```bash
   wrangler publish
   ```

3. 访问 `https://xvdl.your-account.workers.dev/`

## API 文档

### 接口

API 有一个接受 X/Twitter URL 的单一端点：

```
GET /{twitter_url}
```

其中 `{twitter_url}` 是指向包含视频的 X/Twitter 帖子的完整 URL。

### 请求格式

请求是一个简单的 HTTP GET，Twitter URL 作为路径。

### 响应格式

- 成功时：包含直接下载 URL 的纯文本响应
- 出错时：带有适当 HTTP 状态码的错误消息（400 表示无效请求，500 表示服务器错误）

## 技术栈

- [Rust](https://www.rust-lang.org/)
- [Cloudflare Workers](https://workers.cloudflare.com/) -
- [worker-rs](https://github.com/cloudflare/workers-rs) - Cloudflare Workers 的 Rust 绑定
- [reqwest](https://github.com/seanmonstar/reqwest) - Rust HTTP 客户端
- [serde](https://github.com/serde-rs/serde) - Rust 的序列化框架

## 许可证

MIT

## 作者

- Meowu (474384902@qq.com)

## 贡献

欢迎贡献！请随时提交 Pull Request。
