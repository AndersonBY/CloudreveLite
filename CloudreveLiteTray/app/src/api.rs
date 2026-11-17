use crate::util;
use serde::Deserialize;
use std::io::{Read, Seek};
use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, SendBody};

pub const CODE_OBJECT_EXIST: i64 = 40004;
pub const CODE_LOCK_CONFLICT: i64 = 40073;

pub struct ApiError {
    pub code: i64,
    pub msg: String,
}

impl ApiError {
    fn msg(code: i64, m: &str) -> Self {
        ApiError {
            code,
            msg: if m.is_empty() {
                format!("Cloudreve 错误 {code}")
            } else {
                m.to_string()
            },
        }
    }
}

/// PlatformVerifier keeps schannel's system roots + AIA chain building.
/// ureq's default (WebPki roots) forces manual root validation in native-tls,
/// which breaks servers (e.g. OSS) that send only the leaf certificate.
pub fn tls_config() -> TlsConfig {
    TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build()
}

fn api_agent() -> Agent {
    Agent::config_builder()
        .tls_config(tls_config())
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

pub fn transfer_agent() -> Agent {
    Agent::config_builder()
        .tls_config(tls_config())
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .build()
        .new_agent()
}

/// Perform an API call against {server}/api/v4{path} and return the `data`
/// member of the response envelope.
pub fn api_call(
    server: &str,
    key: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<serde_json::Value, ApiError> {
    let url = format!("{}/api/v4{}", server.trim_end_matches('/'), path);
    let mut req = ureq::http::Request::builder().method(method).uri(&url);
    req = req.header("Accept", "application/json");
    if let Some(_b) = body {
        req = req.header("Content-Type", "application/json");
    }
    if !key.is_empty() {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let http_req = req
        .body(body.unwrap_or("").to_string())
        .map_err(|e| ApiError::msg(-1, &e.to_string()))?;
    let mut resp = api_agent()
        .run(http_req)
        .map_err(|e| ApiError::msg(-1, &format!("网络请求失败: {e}")))?;
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if !resp.status().is_success() {
        return Err(ApiError::msg(
            -1,
            &format!(
                "HTTP {}: {}",
                resp.status(),
                text.chars().take(300).collect::<String>()
            ),
        ));
    }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| ApiError::msg(-1, "响应不是有效 JSON"))?;
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
        let detail = v.get("error").and_then(|m| m.as_str()).unwrap_or("");
        let m = if detail.is_empty() {
            msg.to_string()
        } else {
            format!("{msg}: {detail}")
        };
        return Err(ApiError::msg(code, &m));
    }
    Ok(v.get("data").cloned().unwrap_or(serde_json::Value::Null))
}

#[derive(Deserialize)]
struct KeyExchange {
    key: String,
}

/// Exchange a one-time pairing code (crp_...) for an API key (crl_...).
pub fn exchange_pairing(server: &str, code: &str) -> Result<String, ApiError> {
    let body = serde_json::json!({ "code": code.trim() }).to_string();
    let data = api_call(server, "", "POST", "/session/api-key/exchange", Some(&body))?;
    let k: KeyExchange =
        serde_json::from_value(data).map_err(|_| ApiError::msg(-1, "配对响应格式错误"))?;
    if !k.key.starts_with("crl_") {
        return Err(ApiError::msg(-1, "服务器返回的 API Key 无效"));
    }
    Ok(k.key)
}

/// Cheap authorized request used to validate a stored key.
pub fn validate_key(server: &str, key: &str) -> Result<(), ApiError> {
    let uri = util::percent_encode_query("cloudreve://my/");
    api_call(server, key, "GET", &format!("/file?uri={uri}"), None).map(|_| ())
}

#[derive(Clone, Deserialize)]
pub struct FileEntry {
    #[serde(rename = "type")]
    pub file_type: i32,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Deserialize, Default)]
struct FilePagination {
    #[serde(default)]
    page: usize,
    #[serde(default)]
    total_items: Option<usize>,
    #[serde(default)]
    next_token: Option<String>,
}

#[derive(Deserialize)]
struct FileListResponse {
    #[serde(default)]
    files: Vec<FileEntry>,
    #[serde(default)]
    pagination: FilePagination,
}

pub fn list_files(server: &str, key: &str, uri: &str) -> Result<Vec<FileEntry>, ApiError> {
    let encoded = util::percent_encode_query(uri);
    let mut files = Vec::new();
    let mut page = None;
    let mut next_token: Option<String> = None;
    for _ in 0..10_000 {
        let mut path =
            format!("/file?uri={encoded}&page_size=100&order_by=name&order_direction=asc");
        if let Some(token) = next_token.take() {
            path.push_str("&next_page_token=");
            path.push_str(&util::percent_encode_query(&token));
        } else if let Some(number) = page {
            path.push_str(&format!("&page={number}"));
        }

        let data = api_call(server, key, "GET", &path, None)?;
        let response: FileListResponse = serde_json::from_value(data)
            .map_err(|e| ApiError::msg(-1, &format!("文件列表响应格式错误: {e}")))?;
        let before = files.len();
        let fetched = response.files.len();
        files.extend(response.files);
        if let Some(token) = response.pagination.next_token.filter(|v| !v.is_empty()) {
            next_token = Some(token);
            continue;
        }

        if let Some(total) = response.pagination.total_items {
            if files.len() >= total || files.len() == before {
                break;
            }
        } else if fetched < 100 || files.len() == before {
            break;
        }
        page = Some(response.pagination.page + 1);
    }
    Ok(files)
}

pub fn delete_files(server: &str, key: &str, uris: &[String]) -> Result<(), ApiError> {
    let body = serde_json::json!({ "uris": uris }).to_string();
    api_call(server, key, "DELETE", "/file", Some(&body)).map(|_| ())
}

#[derive(Deserialize)]
pub struct StoragePolicy {
    #[serde(rename = "type")]
    pub policy_type: String,
    /// `omitempty` on the server: the field is absent when false (direct upload).
    #[serde(default)]
    pub relay: bool,
}

#[derive(Deserialize)]
pub struct UploadSession {
    #[serde(rename = "session_id")]
    pub session_id: String,
    #[serde(rename = "chunk_size", default)]
    pub chunk_size: i64,
    #[serde(rename = "upload_urls", default)]
    pub upload_urls: Vec<String>,
    #[serde(rename = "completeURL", default)]
    pub complete_url: String,
    #[serde(default)]
    pub uri: String,
    #[serde(rename = "callback_secret", default)]
    pub callback_secret: String,
    #[serde(rename = "storage_policy", default)]
    pub storage_policy: Option<StoragePolicy>,
}

pub fn create_upload_session(
    server: &str,
    key: &str,
    uri: &str,
    size: i64,
    last_modified_ms: i64,
    mime: &str,
    entity_type: &str,
) -> Result<UploadSession, ApiError> {
    let body = serde_json::json!({
        "uri": uri,
        "size": size,
        "last_modified": last_modified_ms,
        "mime_type": mime,
        "entity_type": entity_type,
    })
    .to_string();
    let data = api_call(server, key, "PUT", "/file/upload", Some(&body))?;
    match serde_json::from_value::<UploadSession>(data.clone()) {
        Ok(s) => Ok(s),
        Err(e) => {
            // The session exists server-side even when we cannot parse the
            // response; cancel it so it does not linger as an orphan lock.
            let sid = data
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let suri = data
                .get("uri")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            util::log(&format!(
                "session response parse failed: {e}; raw: {}",
                serde_json::to_string(&data)
                    .map(|s| s.chars().take(600).collect::<String>())
                    .unwrap_or_default()
            ));
            if !sid.is_empty() {
                let dummy = UploadSession {
                    session_id: sid,
                    uri: suri,
                    chunk_size: 0,
                    upload_urls: vec![],
                    complete_url: String::new(),
                    callback_secret: String::new(),
                    storage_policy: None,
                };
                cancel_upload_session(server, key, &dummy);
            }
            Err(ApiError::msg(-2, &format!("上传会话响应解析失败: {e}")))
        }
    }
}

pub fn cancel_upload_session(server: &str, key: &str, session: &UploadSession) {
    let body = serde_json::json!({
        "id": session.session_id,
        "uri": session.uri,
    })
    .to_string();
    let _ = api_call(server, key, "DELETE", "/file/upload", Some(&body));
}

pub fn create_share(
    server: &str,
    key: &str,
    uri: &str,
    days: u32,
    downloads: u32,
    private: bool,
) -> Result<String, ApiError> {
    let body = serde_json::json!({
        "uri": uri,
        "is_private": private,
        "password": "",
        "downloads": downloads,
        "expire": days as i64 * 86400,
    })
    .to_string();
    let data = api_call(server, key, "PUT", "/share", Some(&body))?;
    match data {
        serde_json::Value::String(s) => Ok(s),
        _ => Err(ApiError::msg(-1, "分享链接响应格式错误")),
    }
}

// ---------------------------------------------------------- transfer layer ---

fn read_body_limited(resp: &mut ureq::http::Response<ureq::Body>, limit: usize) -> String {
    let mut buf = [0u8; 8192];
    let mut out: Vec<u8> = Vec::new();
    let mut reader = resp.body_mut().as_reader();
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.len() < limit {
                    out.extend_from_slice(&buf[..n]);
                }
            }
        }
    }
    String::from_utf8_lossy(&out).trim().to_string()
}

/// Reader over one chunk that reports bytes to the progress window and
/// aborts promptly when the user cancels.
struct CountingSection<'a> {
    inner: std::io::Take<&'a std::fs::File>,
}

impl std::io::Read for CountingSection<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if crate::progress::cancel_requested() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let n = self.inner.read(buf)?;
        crate::progress::add_sent(n as u64);
        Ok(n)
    }
}

/// PUT one chunk to a presigned URL. Streams from `file` starting at its
/// current position, reading exactly `len` bytes. Returns the response ETag.
pub fn put_part(
    agent: &Agent,
    file: &mut std::fs::File,
    len: u64,
    url: &str,
) -> Result<Option<String>, String> {
    let start = file.stream_position().unwrap_or(0);
    // byte counter value before this chunk started; retries reset to it
    let base = crate::progress::sent();
    let mut last_err = String::new();
    for attempt in 0..4u32 {
        if attempt > 0 {
            util::log(&format!(
                "upload part retry: attempt={} delay_secs={}",
                attempt + 1,
                1 << (attempt - 1),
            ));
            std::thread::sleep(Duration::from_secs(1 << (attempt - 1)));
            if let Err(e) = file.seek(std::io::SeekFrom::Start(start)) {
                return Err(format!("seek failed: {e}"));
            }
        }
        if crate::progress::cancel_requested() {
            return Err("已取消".into());
        }
        // Re-create the section reader per attempt; reset progress to the
        // chunk start so retries do not inflate the byte counter.
        crate::progress::set_sent(base);
        let mut section = CountingSection {
            inner: (&*file).take(len),
        };
        let req = ureq::http::Request::builder()
            .method("PUT")
            .uri(url)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", len.to_string())
            .body(SendBody::from_reader(&mut section))
            .map_err(|e| e.to_string())?;
        match agent.run(req) {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                if (200..300).contains(&status) {
                    let etag = resp
                        .headers()
                        .get("ETag")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    let _ = read_body_limited(&mut resp, 64 * 1024);
                    return Ok(etag);
                }
                let text = read_body_limited(&mut resp, 64 * 1024);
                if status == 429 || status >= 500 {
                    last_err = format!(
                        "HTTP {status}: {}",
                        text.chars().take(200).collect::<String>()
                    );
                    continue;
                }
                return Err(format!(
                    "HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            Err(e) => {
                last_err = format!("网络错误: {e}");
            }
        }
    }
    Err(last_err)
}

fn post_url(agent: &Agent, url: &str, body: Vec<u8>, oss_complete: bool) -> Result<(), String> {
    let mut builder = ureq::http::Request::builder()
        .method("POST")
        .uri(url)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", body.len().to_string());
    if oss_complete {
        builder = builder
            .header("x-oss-forbid-overwrite", "true")
            .header("x-oss-complete-all", "yes");
    }
    let req = builder.body(body).map_err(|e| e.to_string())?;
    let mut resp = agent.run(req).map_err(|e| format!("网络错误: {e}"))?;
    let status = resp.status().as_u16();
    let text = read_body_limited(&mut resp, 256 * 1024);
    if !(200..300).contains(&status) {
        return Err(format!(
            "HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    if !oss_complete && text.contains("<Error") {
        return Err(format!(
            "S3 返回错误文档: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    Ok(())
}

pub fn complete_oss(agent: &Agent, url: &str) -> Result<(), String> {
    post_url(agent, url, Vec::new(), true)
}

pub fn complete_s3(agent: &Agent, url: &str, parts: &[(u32, String)]) -> Result<(), String> {
    let mut xml = String::from("<CompleteMultipartUpload>");
    for (num, etag) in parts {
        let esc = etag
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        xml.push_str(&format!(
            "<Part><PartNumber>{num}</PartNumber><ETag>{esc}</ETag></Part>"
        ));
    }
    xml.push_str("</CompleteMultipartUpload>");
    post_url(agent, url, xml.into_bytes(), false)
}

pub fn s3_callback(server: &str, session_id: &str, secret: &str) -> Result<(), ApiError> {
    let path = format!(
        "/callback/s3/{}/{}",
        util::percent_encode_query(session_id),
        util::percent_encode_query(secret)
    );
    api_call(server, "", "GET", &path, None).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TRANSFER_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn multipart_requests_reuse_connection() {
        let _guard = TRANSFER_TEST.lock().unwrap();
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut connections = 0;
            let mut bodies = Vec::new();
            while bodies.len() < 5 {
                assert!(std::time::Instant::now() < deadline, "missing request");
                let (stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept failed: {e}"),
                };
                connections += 1;
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                while bodies.len() < 5 {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        break;
                    }
                    assert!(line.starts_with("PUT ") || line.starts_with("POST "));
                    let mut len = None;
                    loop {
                        line.clear();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        assert!(!line.to_ascii_lowercase().starts_with("authorization:"));
                        if let Some((name, value)) = line.split_once(':') {
                            if name.eq_ignore_ascii_case("content-length") {
                                len = Some(value.trim().parse::<usize>().unwrap());
                            }
                        }
                    }
                    let mut body = vec![0; len.expect("Content-Length")];
                    reader.read_exact(&mut body).unwrap();
                    bodies.push(body);
                    reader
                        .get_mut()
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"part\"\r\n\r\n",
                        )
                        .unwrap();
                }
            }
            (connections, bodies)
        });
        let path = std::env::temp_dir().join(format!(
            "cloudreve-tray-parts-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"0123456789").unwrap();
        file.rewind().unwrap();
        crate::progress::set_sent(0);
        let agent = transfer_agent();
        for len in [4, 4, 2] {
            assert_eq!(
                put_part(&agent, &mut file, len, &url).unwrap().as_deref(),
                Some("\"part\"")
            );
        }
        complete_oss(&agent, &url).unwrap();
        complete_s3(&agent, &url, &[(1, "\"part\"".into())]).unwrap();
        let sent = crate::progress::sent();
        drop(file);
        std::fs::remove_file(path).unwrap();
        let (connections, bodies) = server.join().unwrap();
        assert_eq!(sent, 10);
        assert_eq!(
            &bodies[..4],
            &[b"0123".to_vec(), b"4567".to_vec(), b"89".to_vec(), vec![]]
        );
        assert!(String::from_utf8_lossy(&bodies[4]).contains("<ETag>\"part\"</ETag>"));
        assert_eq!(
            connections, 1,
            "parts and completion must share the connection"
        );
    }

    #[test]
    fn forbidden_part_is_not_retried_as_a_network_error() {
        use std::io::{BufRead, BufReader, Write};
        let _guard = TRANSFER_TEST.lock().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/part", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = [0u8; 3];
            reader.read_exact(&mut body).unwrap();
            assert_eq!(&body, b"abc");
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let path = std::env::temp_dir().join(format!(
            "cloudreve-forbidden-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"abc").unwrap();
        file.rewind().unwrap();
        let result = put_part(&transfer_agent(), &mut file, 3, &url);
        drop(file);
        std::fs::remove_file(path).unwrap();
        server.join().unwrap();
        assert!(result.unwrap_err().starts_with("HTTP 403:"));
    }

    /// The server serializes `relay` with omitempty: it is absent when false.
    #[test]
    fn parse_session_without_relay() {
        let j = serde_json::json!({
            "session_id": "s1",
            "upload_id": "u1",
            "chunk_size": 10485760,
            "expires": 1787400000i64,
            "upload_urls": ["https://bucket.oss.example.com/put"],
            "completeURL": "https://bucket.oss.example.com/complete",
            "uri": "cloudreve://my/a.bin",
            "callback_secret": "sec",
            "storage_policy": {"id": "1", "name": "p", "type": "oss", "max_size": 1000}
        });
        let s: UploadSession = serde_json::from_value(j).expect("parse");
        let p = s.storage_policy.expect("policy");
        assert_eq!(p.policy_type, "oss");
        assert!(!p.relay);
        assert_eq!(s.upload_urls.len(), 1);
    }
}
