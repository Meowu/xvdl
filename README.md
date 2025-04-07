# XVDL - X/Twitter Video Downloader

[English](README.md) | [简体中文](README_zh.md)

A Rust-based Cloudflare Workers service that generates direct download links for videos from X.com (formerly Twitter).

## Features

- Generate direct download links for X.com/Twitter videos
- Fast and efficient using Rust and Cloudflare Workers
- Simple REST API with easy integration
- Serverless deployment via Cloudflare Workers
- Handles both x.com and twitter.com URLs

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) and Cargo
- [Node.js](https://nodejs.org/) and npm (for Cloudflare Wrangler)
- [Wrangler CLI](https://developers.cloudflare.com/workers/wrangler/install-and-update/) (Cloudflare Workers CLI)

### Setup

1. Clone the repository:
   ```bash
   git clone https://github.com/yourusername/xvdl.git
   cd xvdl
   ```

2. Install dependencies:
   ```bash
   cargo build
   ```

3. Install the worker-build tool:
   ```bash
   cargo install worker-build
   ```

## Usage

Send an HTTP request to your deployed Cloudflare Worker with the X/Twitter URL path:

```
https://your-worker-subdomain.workers.dev/https://x.com/username/status/1234567890
```

The service will return a direct download URL to the video if successful.

### Example

Request:
```
GET https://your-worker-subdomain.workers.dev/https://x.com/username/status/1234567890
```

Successful Response:
```
https://video-host.com/path/to/video.mp4
```

Error Response (400 Bad Request):

```
Invalid X URL. Only x.com and twitter.com URLs are supported.
```

Error Response (500 Internal Server Error):

```
Download job failed
```

## Deployment

To deploy the service to Cloudflare Workers:

1. Log in to Cloudflare using Wrangler:

   ```bash
   wrangler login
   ```

2. Build and deploy the service:

   ```bash
   wrangler publish
   ```

3. Your service will be available at `https://xvdl.your-account.workers.dev/`


## API Documentation

### Endpoint

The API has a single endpoint that accepts X/Twitter URLs:

```
GET /{twitter_url}
```

Where `{twitter_url}` is a complete URL to an X/Twitter post containing a video.

### Request Format

The request is a simple HTTP GET with the Twitter URL as the path.

### Response Format

- On success: A plain text response containing the direct download URL
- On error: An error message with appropriate HTTP status code (400 for invalid requests, 500 for server errors)

## Dependencies and Technologies

- [Rust](https://www.rust-lang.org/) - Programming language
- [Cloudflare Workers](https://workers.cloudflare.com/) - Serverless platform
- [worker-rs](https://github.com/cloudflare/workers-rs) - Rust bindings for Cloudflare Workers
- [reqwest](https://github.com/seanmonstar/reqwest) - HTTP client for Rust
- [serde](https://github.com/serde-rs/serde) - Serialization framework for Rust

## License

MIT


## Author

- Meowu (474384902@qq.com)

## Contribution

Contributions are welcome! Please feel free to submit a Pull Request.
