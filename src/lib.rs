use gloo_timers::future::sleep;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use worker::*;

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DownloadStatus {
    Error,
    Finished,
}

#[derive(Serialize)]
struct ValidateRequest {
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ValidateResponse {
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RequestResponse {
    #[serde(rename = "_id")]
    job_id: String,
}

#[derive(Debug, Deserialize)]
struct DownloadResponse {
    status: DownloadStatus,
    host: Option<String>,
    filename: Option<String>,
}

async fn handler(video_url: &str) -> Result<String> {
    let validate_url = "https://api.x-downloader.com/validate";
    let body = ValidateRequest {
        url: video_url.to_string(),
    };
    let client = reqwest::Client::new();

    let validate_resp = client
        .post(validate_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::RustError(format!("Failed to parse validation response:: {}", e)))?;

    let validate_data: ValidateResponse = validate_resp
        .json()
        .await
        .map_err(|e| Error::RustError(format!("Faile to parse validation response: {}", e)))?;

    if validate_data.status == "error" {
        return Err(Error::RustError("Invalid x video url.".to_string()));
    }

    let request_url = "https://api.x-downloader.com/request";

    let resquest_resp = client
        .post(request_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::RustError(format!("Request job failed: {}", e)))?;

    let request_data: RequestResponse = resquest_resp
        .json()
        .await
        .map_err(|e| Error::RustError(format!("Failed to parse request response: {}", e)))?;

    let job_id = request_data.job_id;

    // poll for job status.
    // for _ in 0..30 {}
    let file_url = loop {
        let download_api = "https://api.x-downloader.com/download";
        let download_resp = client
            .get(format!("{}/{}", download_api, job_id))
            .send()
            .await
            .map_err(|e| Error::RustError(format!("Download status check failed: {}", e)))?;

        let download_data: DownloadResponse = download_resp
            .json()
            .await
            .map_err(|e| Error::RustError(format!("Failed to parse download response: {}", e)))?;

        if download_data.status == DownloadStatus::Error {
            return Err(Error::RustError("Download job failed".to_string()));
        }

        if download_data.status == DownloadStatus::Finished {
            if let (Some(host), Some(filename)) = (download_data.host, download_data.filename) {
                break format!("https://{}/{}", host, filename);
            } else {
                return Err(Error::RustError(
                    "Download finished but host or filename missing".to_string(),
                ));
            }
        }

        sleep(Duration::from_secs(2)).await;
    };

    Ok(file_url)
}

#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let path = req.path().to_string();
    let video_url = path.strip_prefix('/').unwrap_or(&path);
    if video_url.is_empty() {
        return Response::error("No URL privided", 400);
    }

    if !video_url.contains("x.com") && !video_url.contains("twitter.com") {
        return Response::error(
            "Invalid X URL. Only x.com and twitter.com URLs are supported.",
            400,
        );
    }
    println!("Video url: {}", video_url);
    match handler(video_url).await {
        Ok(result) => Response::ok(result),
        Err(err) => Response::error(err.to_string(), 500),
    }
}
