use serde::Deserialize;
use worker::*;

const FXTWITTER_API: &str = "https://api.fxtwitter.com/status";

#[derive(Debug, Deserialize)]
struct FxTwitterResponse {
    code: u16,
    message: String,
    tweet: Option<Tweet>,
}

#[derive(Debug, Deserialize)]
struct Tweet {
    media: Option<Media>,
}

#[derive(Debug, Deserialize)]
struct Media {
    #[serde(default)]
    videos: Vec<Video>,
}

#[derive(Debug, Deserialize)]
struct Video {
    url: String,
}

fn status_id(video_url: &str) -> Option<&str> {
    let (_, tail) = video_url.split_once("/status/")?;
    let id = tail
        .split(|character| matches!(character, '/' | '?' | '#'))
        .next()?;

    (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())).then_some(id)
}

async fn video_urls(video_url: &str) -> Result<Vec<String>> {
    let id =
        status_id(video_url).ok_or_else(|| Error::RustError("Invalid X status URL".to_string()))?;
    let client = reqwest::Client::builder()
        .user_agent("xvdl/0.1.0 (https://github.com/Meowu/xvdl)")
        .build()
        .map_err(|error| Error::RustError(format!("Failed to build HTTP client: {error}")))?;

    let response = client
        .get(format!("{FXTWITTER_API}/{id}"))
        .send()
        .await
        .map_err(|error| Error::RustError(format!("Failed to fetch post metadata: {error}")))?;
    let http_status = response.status();
    let response_body = response
        .text()
        .await
        .map_err(|error| Error::RustError(format!("Failed to read metadata response: {error}")))?;

    if !http_status.is_success() {
        return Err(Error::RustError(format!(
            "Metadata service returned HTTP {http_status}: {response_body}"
        )));
    }

    let data: FxTwitterResponse = serde_json::from_str(&response_body).map_err(|error| {
        Error::RustError(format!(
            "Failed to parse metadata response: {error}; response: {response_body}"
        ))
    })?;

    if data.code != 200 {
        return Err(Error::RustError(format!(
            "Metadata service failed ({}): {}",
            data.code, data.message
        )));
    }

    let urls = data
        .tweet
        .and_then(|tweet| tweet.media)
        .map(|media| {
            media
                .videos
                .into_iter()
                .map(|video| video.url)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if urls.is_empty() {
        return Err(Error::RustError("No videos found in this post".to_string()));
    }

    Ok(urls)
}

#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let path = req.path();
    let video_url = path.strip_prefix('/').unwrap_or(&path);

    if video_url.is_empty() {
        return Response::error("No URL provided", 400);
    }

    if !video_url.contains("x.com") && !video_url.contains("twitter.com") {
        return Response::error(
            "Invalid X URL. Only x.com and twitter.com URLs are supported.",
            400,
        );
    }

    match video_urls(video_url).await {
        Ok(urls) => Response::from_json(&urls),
        Err(error) => Response::error(error.to_string(), 502),
    }
}

#[cfg(test)]
mod tests {
    use super::status_id;

    #[test]
    fn extracts_status_id_with_query_string() {
        assert_eq!(
            status_id("https://x.com/DuoHong44622/status/2094259883487133792?s=20"),
            Some("2094259883487133792")
        );
    }

    #[test]
    fn rejects_non_status_urls_and_non_numeric_ids() {
        assert_eq!(status_id("https://x.com/DuoHong44622"), None);
        assert_eq!(status_id("https://x.com/user/status/not-an-id"), None);
    }
}
