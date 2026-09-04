use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::{PostReference, XReadError};

static EMBEDDED_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)https?://[^\s<>\"']+"#).expect("valid URL regex"));

/// Accept a status ID, a normal X/Twitter URL, or text containing such a URL.
pub fn parse_post_reference(input: &str) -> Result<PostReference, XReadError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(XReadError::invalid_input("缺少 X 推文链接或推文 ID。")
            .with_hint("例如：xread https://x.com/Interior/status/463440424141459456"));
    }

    if is_status_id(trimmed) {
        return Ok(make_reference(trimmed, None, trimmed));
    }

    let embedded = EMBEDDED_URL
        .find(trimmed)
        .map(|found| found.as_str())
        .unwrap_or(trimmed);
    let cleaned = embedded.trim_end_matches(['>', ',', '.', ';', ')']);
    let lower = cleaned.to_ascii_lowercase();
    let candidate = if lower.starts_with("x.com/")
        || lower.starts_with("www.x.com/")
        || lower.starts_with("mobile.x.com/")
        || lower.starts_with("twitter.com/")
        || lower.starts_with("www.twitter.com/")
        || lower.starts_with("mobile.twitter.com/")
    {
        format!("https://{cleaned}")
    } else {
        cleaned.to_owned()
    };

    let url = Url::parse(&candidate).map_err(|_| invalid_reference_error())?;
    let hostname = url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(invalid_reference_error)?;
    if !is_x_hostname(&hostname) {
        return Err(invalid_reference_error());
    }

    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    let status_index = segments
        .iter()
        .position(|part| {
            part.eq_ignore_ascii_case("status") || part.eq_ignore_ascii_case("statuses")
        })
        .ok_or_else(invalid_reference_error)?;
    let id = segments
        .get(status_index + 1)
        .filter(|id| is_status_id(id))
        .ok_or_else(invalid_reference_error)?;
    let username = segments
        .first()
        .filter(|part| !part.eq_ignore_ascii_case("i"))
        .map(|part| (*part).to_owned());

    Ok(make_reference(id, username, trimmed))
}

fn make_reference(id: &str, username: Option<String>, original: &str) -> PostReference {
    PostReference {
        id: id.to_owned(),
        original: original.to_owned(),
        url: format!(
            "https://x.com/{}/status/{id}",
            username.as_deref().unwrap_or("i")
        ),
        username,
    }
}

fn is_status_id(value: &str) -> bool {
    (1..=19).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_x_hostname(hostname: &str) -> bool {
    hostname == "x.com"
        || hostname.ends_with(".x.com")
        || hostname == "twitter.com"
        || hostname.ends_with(".twitter.com")
}

fn invalid_reference_error() -> XReadError {
    XReadError::invalid_input("无法识别这个 X 推文链接或 ID。")
        .with_hint("支持 x.com/<用户>/status/<ID>、twitter.com/<用户>/status/<ID> 或纯数字 ID。")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_id_and_common_url_forms() {
        let id = "463440424141459456";
        assert_eq!(parse_post_reference(id).unwrap().id, id);

        let reference = parse_post_reference(&format!(
            "看看这个：https://twitter.com/Interior/status/{id}?s=20)"
        ))
        .unwrap();
        assert_eq!(reference.id, id);
        assert_eq!(reference.username.as_deref(), Some("Interior"));
        assert_eq!(reference.url, format!("https://x.com/Interior/status/{id}"));

        assert_eq!(
            parse_post_reference(&format!("x.com/i/status/{id}"))
                .unwrap()
                .username,
            None
        );
    }

    #[test]
    fn rejects_lookalike_hosts_and_non_numeric_ids() {
        assert!(parse_post_reference("https://notx.com/user/status/123").is_err());
        assert!(parse_post_reference("https://x.com/user/status/nope").is_err());
    }
}
