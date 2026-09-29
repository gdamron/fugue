//! Blocking HTTP helpers shared by the native (non-wasm) fetch paths.
//!
//! Wraps `ureq` 3 so every caller keeps the behavior `ureq` 2 had:
//! - 4xx/5xx fail with `"<url>: status code <code> <reason>"`. `ureq` 3 would
//!   report a bare `http status: 404`, so the agent hands the response back
//!   and [`check_status`] builds the message. The body is not read, so an
//!   error returns as soon as the headers arrive.
//! - Connecting times out after 30 s (`ureq` 3 has no connect timeout by
//!   default).
//! - [`read_text`] caps the decoded body at 10 MB and replaces invalid UTF-8,
//!   like `ureq` 2's `into_string`.

use std::io::Read;
use std::time::Duration;

use ureq::http::Response;
use ureq::{Agent, Body};

/// Cap on the decoded bytes buffered into a `String`, matching the limit
/// `ureq` 2 applied in `Response::into_string`.
pub(crate) const MAX_TEXT_BODY_BYTES: u64 = 10 * 1024 * 1024;

/// `ureq` 2's default connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
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

/// Reads the whole body as text. Fails when the decoded body is larger than
/// [`MAX_TEXT_BODY_BYTES`]; invalid UTF-8 is replaced rather than rejected.
///
/// The cap is applied to the decoded reader: `ureq` 3's `BodyWithConfig::limit`
/// counts bytes before gzip decoding, so a small compressed body could still
/// expand without bound.
pub(crate) fn read_text(body: &mut Body) -> Result<String, String> {
    let mut bytes = Vec::new();
    body.as_reader()
        .take(MAX_TEXT_BODY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    if bytes.len() as u64 > MAX_TEXT_BODY_BYTES {
        return Err(format!(
            "response body exceeds {} bytes",
            MAX_TEXT_BODY_BYTES
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn check_status(url: &str, response: Response<Body>) -> Result<Response<Body>, String> {
    let status = response.status();
    if !status.is_client_error() && !status.is_server_error() {
        return Ok(response);
    }
    let mut message = format!("{url}: status code {}", status.as_u16());
    if let Some(reason) = status.canonical_reason() {
        message.push(' ');
        message.push_str(reason);
    }
    Err(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// Serves one canned response on a loopback port and returns its URL.
    fn serve_once(status_line: &'static str, content_type: &'static str, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
                // Drain what the client still sends (e.g. a POST body) so
                // closing doesn't reset the connection before it reads the reply.
                let _ = stream.shutdown(std::net::Shutdown::Write);
                let _ = std::io::copy(&mut stream, &mut std::io::sink());
            }
        });
        format!("http://{addr}/thing")
    }

    #[test]
    fn get_returns_body_on_success() {
        let url = serve_once("200 OK", "text/plain", b"hello".to_vec());
        let mut response = get(&url).unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(read_text(response.body_mut()).unwrap(), "hello");
    }

    #[test]
    fn get_reports_url_and_status_on_http_error() {
        let url = serve_once("404 Not Found", "text/plain", b"no such sample".to_vec());
        assert_eq!(
            get(&url).unwrap_err(),
            format!("{url}: status code 404 Not Found")
        );
    }

    #[test]
    fn post_reports_url_and_status_on_http_error() {
        let url = serve_once("401 Unauthorized", "application/json", b"{}".to_vec());
        let err = post(&url, &[("content-type", "application/json")], "{}").unwrap_err();
        assert_eq!(err, format!("{url}: status code 401 Unauthorized"));
    }

    #[test]
    fn read_text_replaces_invalid_utf8() {
        let url = serve_once("200 OK", "text/plain", vec![b'o', b'k', 0xff]);
        let mut response = get(&url).unwrap();
        assert_eq!(read_text(response.body_mut()).unwrap(), "ok\u{fffd}");
    }

    #[test]
    fn read_text_rejects_bodies_over_the_cap() {
        let body = vec![b'a'; MAX_TEXT_BODY_BYTES as usize + 1];
        let url = serve_once("200 OK", "text/plain", body);
        let mut response = get(&url).unwrap();
        let err = read_text(response.body_mut()).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
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
