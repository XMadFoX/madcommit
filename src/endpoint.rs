use url::{Host, Url};

#[derive(Debug)]
pub struct EndpointError(pub String);

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EndpointError {}

/// Normalize an OpenAI-compatible base URL for storage and requests.
///
/// The path is kept (trailing slash added) so keys for `/v1/` and another
/// prefix never collide. Userinfo, query, and fragment are rejected so a
/// credential cannot ride along in the URL. HTTP is allowed only for
/// loopback hosts.
pub fn normalize_endpoint(raw: &str) -> Result<String, EndpointError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(EndpointError(
            "API endpoint is empty. Set endpoint in config or pass --endpoint.".to_string(),
        ));
    }

    let url = Url::parse(trimmed)
        .map_err(|_| EndpointError("API endpoint must be an absolute URL.".to_string()))?;

    match url.scheme() {
        "https" => {}
        "http" => {
            if !is_loopback(&url) {
                return Err(EndpointError(
                    "HTTP endpoints are only allowed for localhost. Use HTTPS.".to_string(),
                ));
            }
        }
        _ => {
            return Err(EndpointError(
                "API endpoint must use https (or http on localhost).".to_string(),
            ));
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(EndpointError(
            "API endpoint must not include username or password. Store the API key with `madcommit auth login` or OPENAI_API_KEY.".to_string(),
        ));
    }

    if url.query().is_some() {
        return Err(EndpointError(
            "API endpoint must not include a query string.".to_string(),
        ));
    }

    if url.fragment().is_some() {
        return Err(EndpointError(
            "API endpoint must not include a URL fragment.".to_string(),
        ));
    }

    if url.host_str().is_none() {
        return Err(EndpointError(
            "API endpoint must include a host.".to_string(),
        ));
    }

    let mut out = url.to_string();
    if !out.ends_with('/') {
        out.push('/');
    }
    Ok(out)
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(addr)) => addr.is_loopback(),
        Some(Host::Ipv6(addr)) => addr.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_openai_url_keeps_path() {
        assert_eq!(
            normalize_endpoint("https://api.openai.com/v1").unwrap(),
            "https://api.openai.com/v1/"
        );
        assert_eq!(
            normalize_endpoint("https://api.openai.com/v1/").unwrap(),
            "https://api.openai.com/v1/"
        );
    }

    #[test]
    fn different_paths_are_distinct() {
        let a = normalize_endpoint("https://example.com/v1").unwrap();
        let b = normalize_endpoint("https://example.com/v1beta").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn lowercases_host() {
        assert_eq!(
            normalize_endpoint("https://API.OpenAI.com/v1/").unwrap(),
            "https://api.openai.com/v1/"
        );
    }

    #[test]
    fn rejects_userinfo() {
        let err = normalize_endpoint("https://user:sk-secret@api.openai.com/v1/").unwrap_err();
        assert!(err.0.contains("username or password"), "{err}");
    }

    #[test]
    fn rejects_query_and_fragment() {
        assert!(normalize_endpoint("https://api.openai.com/v1/?foo=bar").is_err());
        assert!(normalize_endpoint("https://api.openai.com/v1/#frag").is_err());
    }

    #[test]
    fn rejects_http_remote() {
        let err = normalize_endpoint("http://api.openai.com/v1/").unwrap_err();
        assert!(err.0.contains("localhost"), "{err}");
    }

    #[test]
    fn allows_http_loopback() {
        assert_eq!(
            normalize_endpoint("http://127.0.0.1:11434/v1").unwrap(),
            "http://127.0.0.1:11434/v1/"
        );
        assert_eq!(
            normalize_endpoint("http://localhost:8080/v1/").unwrap(),
            "http://localhost:8080/v1/"
        );
        assert_eq!(
            normalize_endpoint("http://[::1]/v1").unwrap(),
            "http://[::1]/v1/"
        );
    }

    #[test]
    fn rejects_relative_and_empty() {
        assert!(normalize_endpoint("").is_err());
        assert!(normalize_endpoint("api.openai.com/v1").is_err());
    }

    #[test]
    fn errors_omit_raw_input() {
        for raw in [
            "dummy-secret-not-a-url",
            "https://user:dummy-secret@api.openai.com/v1/",
            "dummy-secret://api.openai.com/v1/",
            "https://api.openai.com/v1/?api_key=dummy-secret",
        ] {
            let err = normalize_endpoint(raw).unwrap_err().to_string();
            assert!(!err.contains("dummy-secret"), "{err}");
            assert!(!err.contains(raw), "{err}");
        }
    }
}
