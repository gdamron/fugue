//! Blocking HTTP helpers shared by the native (non-wasm) fetch paths.
//!
//! Wraps `ureq` 3 so every caller reports HTTP failures the same way.
//! `ureq` 3 surfaces 4xx/5xx as a bare `http status: 404` error with no URL or
//! body; here the agent hands back the response instead, and [`check_status`]
//! turns it into `"<url>: status code <code> <reason>: <body preview>"`. The
//! URL prefix and `status code` wording match what `ureq` 2 reported, and the
//! body preview adds the provider's own error message when there is one.

use std::io::Read;

use ureq::http::Response;
use ureq::{Agent, Body};

/// Cap for responses buffered into a `String`, matching the limit `ureq` 2
/// applied to `Response::into_string`.
pub(crate) const MAX_TEXT_BODY_BYTES: u64 = 10 * 1024 * 1024;

/// How much of an HTTP error response body is echoed into the error message.
const ERROR_BODY_PREVIEW_BYTES: u64 = 512;

fn agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

/// Issues a GET request; 4xx/5xx responses become `Err`.
pub(crate) fn get(url: &str) -> Result<Response<Body>, String> {
    let response = agent()
        .get(url)
        .call()
        .map_err(|err| format!("{url}: {err}"))?;
    check_status(url, response)
}

/// Issues a POST request with `body` as the payload; 4xx/5xx responses become `Err`.
pub(crate) fn post(
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<Response<Body>, String> {
    let mut request = agent().post(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send(body).map_err(|err| format!("{url}: {err}"))?;
    check_status(url, response)
}

/// Reads the whole body as UTF-8 text, failing past [`MAX_TEXT_BODY_BYTES`]
/// or on invalid UTF-8 (as `ureq` 2's `into_string` did).
pub(crate) fn read_text(body: &mut Body) -> Result<String, String> {
    body.with_config()
        .limit(MAX_TEXT_BODY_BYTES)
        .read_to_string()
        .map_err(|err| err.to_string())
}

fn check_status(url: &str, mut response: Response<Body>) -> Result<Response<Body>, String> {
    let status = response.status();
    if !status.is_client_error() && !status.is_server_error() {
        return Ok(response);
    }
    let mut message = format!("{url}: status code {}", status.as_u16());
    if let Some(reason) = status.canonical_reason() {
        message.push(' ');
        message.push_str(reason);
    }
    // HTML error pages (CDN/web-server 404s) are noise; API errors are JSON or plain text.
    let is_html = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("html"));
    if !is_html {
        let mut preview = Vec::new();
        let _ = response
            .body_mut()
            .as_reader()
            .take(ERROR_BODY_PREVIEW_BYTES)
            .read_to_end(&mut preview);
        let preview = String::from_utf8_lossy(&preview);
        let preview = preview.trim();
        if !preview.is_empty() {
            message.push_str(": ");
            message.push_str(preview);
        }
    }
    Err(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// Serves one canned response on a loopback port and returns its URL.
    fn serve_once(status_line: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}/thing")
    }

    #[test]
    fn get_returns_body_on_success() {
        let url = serve_once("200 OK", "hello");
        let mut response = get(&url).unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(read_text(response.body_mut()).unwrap(), "hello");
    }

    #[test]
    fn get_reports_status_url_and_body_on_http_error() {
        let url = serve_once("404 Not Found", "no such sample");
        let err = get(&url).unwrap_err();
        assert_eq!(
            err,
            format!("{url}: status code 404 Not Found: no such sample")
        );
    }

    #[test]
    fn get_omits_html_error_pages() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = "<html><body>Not found</body></html>";
                let _ = write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let url = format!("http://{addr}/page");
        assert_eq!(
            get(&url).unwrap_err(),
            format!("{url}: status code 404 Not Found")
        );
    }

    #[test]
    fn get_omits_empty_error_body() {
        let url = serve_once("503 Service Unavailable", "");
        let err = get(&url).unwrap_err();
        assert_eq!(err, format!("{url}: status code 503 Service Unavailable"));
    }

    #[test]
    fn post_reports_provider_error_message() {
        let url = serve_once("401 Unauthorized", "{\"error\":\"bad key\"}");
        let err = post(&url, &[("content-type", "application/json")], "{}").unwrap_err();
        assert!(err.contains("status code 401"), "{err}");
        assert!(err.contains("bad key"), "{err}");
    }

    #[test]
    fn transport_failure_names_the_url() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}/gone");
        let err = get(&url).unwrap_err();
        assert!(err.starts_with(&format!("{url}: ")), "{err}");
    }
}
