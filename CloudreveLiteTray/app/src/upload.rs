use crate::api::{self, ApiError, UploadSession};
use crate::config::Config;
use crate::util;
use std::io::Seek;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const WM_APP_UPLOAD_DONE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
pub const WM_APP_UPLOAD_STATE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 3;
pub const WM_APP_UPLOAD_LINKS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;
pub const WM_APP_NEED_SETUP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 5;

const MAX_PARALLEL_PARTS: usize = 3;

pub struct UploadOutcome {
    pub ok: Vec<String>,
    pub failed: Vec<(String, String)>,
}

#[derive(Default)]
struct Activity {
    batches: usize,
    updating: bool,
}

impl Activity {
    fn begin_update(&mut self) -> bool {
        if self.batches != 0 || self.updating {
            return false;
        }
        self.updating = true;
        true
    }
}

static ACTIVITY: Mutex<Activity> = Mutex::new(Activity {
    batches: 0,
    updating: false,
});

pub fn enqueue(tx: &Sender<Vec<String>>, files: Vec<String>) -> bool {
    let mut activity = ACTIVITY.lock().unwrap();
    if activity.updating {
        return false;
    }
    activity.batches += 1;
    if tx.send(files).is_err() {
        activity.batches -= 1;
        return false;
    }
    true
}

pub fn begin_update() -> bool {
    ACTIVITY.lock().unwrap().begin_update()
}

pub fn abort_update() {
    ACTIVITY.lock().unwrap().updating = false;
}

struct BatchGuard;

#[cfg(test)]
mod activity_tests {
    use super::*;

    #[test]
    fn queued_uploads_block_update_and_update_blocks_new_work() {
        let mut activity = Activity {
            batches: 2,
            updating: false,
        };
        assert!(!activity.begin_update());
        activity.batches = 1;
        assert!(!activity.begin_update());
        activity.batches = 0;
        assert!(activity.begin_update());
        assert!(activity.updating);
        assert!(!activity.begin_update());
    }

    #[test]
    fn enqueue_tracks_waiting_batches_and_rejects_during_installation() {
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(enqueue(&tx, vec!["first".into()]));
        assert!(enqueue(&tx, vec!["second".into()]));
        assert!(!begin_update());
        assert_eq!(rx.recv().unwrap(), vec!["first"]);
        drop(BatchGuard);
        assert!(!begin_update());
        assert_eq!(rx.recv().unwrap(), vec!["second"]);
        drop(BatchGuard);
        assert!(begin_update());
        let accepted = enqueue(&tx, vec!["late".into()]);
        abort_update();
        assert!(!accepted);
        assert!(rx.try_recv().is_err());
    }
}

impl Drop for BatchGuard {
    fn drop(&mut self) {
        ACTIVITY.lock().unwrap().batches -= 1;
    }
}

#[cfg(test)]
mod parallel_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn upload_parts_use_three_concurrent_requests() {
        let _guard = crate::api::TRANSFER_TEST.lock().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/part", listener.local_addr().unwrap());
        let completed = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let completed_server = Arc::clone(&completed);
        let active_server = Arc::clone(&active);
        let max_active_server = Arc::clone(&max_active);

        let server = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while completed_server.load(Ordering::Relaxed) < 6 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "missing parallel requests"
                );
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let completed = Arc::clone(&completed_server);
                        let active = Arc::clone(&active_server);
                        let max_active = Arc::clone(&max_active_server);
                        handlers.push(std::thread::spawn(move || {
                            let mut reader = BufReader::new(stream);
                            loop {
                                if completed.load(Ordering::Relaxed) >= 6 {
                                    break;
                                }
                                let mut line = String::new();
                                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                    break;
                                }
                                let mut length = 0usize;
                                loop {
                                    line.clear();
                                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                        return;
                                    }
                                    if line == "\r\n" {
                                        break;
                                    }
                                    if let Some((name, value)) = line.split_once(':') {
                                        if name.eq_ignore_ascii_case("content-length") {
                                            length = value.trim().parse().unwrap();
                                        }
                                    }
                                }
                                let mut body = vec![0u8; length];
                                reader.read_exact(&mut body).unwrap();
                                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                                max_active.fetch_max(now, Ordering::SeqCst);
                                std::thread::sleep(Duration::from_millis(150));
                                active.fetch_sub(1, Ordering::SeqCst);
                                completed.fetch_add(1, Ordering::SeqCst);
                                reader
                                    .get_mut()
                                    .write_all(
                                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n",
                                    )
                                    .unwrap();
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept failed: {error}"),
                }
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });

        let path = std::env::temp_dir().join(format!(
            "cloudreve-parallel-upload-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&vec![b'x'; 6 * 1024]).unwrap();
        drop(file);
        let urls = vec![url; 6];
        let agent = api::transfer_agent();
        let result = upload_parts_parallel(&path, 6 * 1024, 1024, &urls, &agent, "oss");
        std::fs::remove_file(path).unwrap();
        result.unwrap();
        server.join().unwrap();
        assert_eq!(completed.load(Ordering::Relaxed), 6);
        assert_eq!(max_active.load(Ordering::Relaxed), 3);
    }
}

fn post_str(hwnd: HWND, msg: u32, wparam: usize, text: String) {
    let boxed = Box::into_raw(Box::new(text)) as isize;
    unsafe {
        let _ = PostMessageW(Some(hwnd), msg, WPARAM(wparam), LPARAM(boxed));
    }
}

fn map_api_err(e: ApiError) -> String {
    e.msg
}

/// Create the upload session, retrying with overwrite semantics when the
/// destination name is already taken.
fn open_session(
    cfg: &Config,
    key: &str,
    uri: &str,
    size: i64,
    mtime: i64,
    mime: &str,
) -> Result<UploadSession, String> {
    match api::create_upload_session(&cfg.server, key, uri, size, mtime, mime, "") {
        Ok(s) => Ok(s),
        Err(e) if e.code == api::CODE_OBJECT_EXIST => {
            api::create_upload_session(&cfg.server, key, uri, size, mtime, mime, "version")
                .map_err(map_api_err)
        }
        Err(e) if e.code == api::CODE_LOCK_CONFLICT => Err(
            "该文件已有一个未完成的上传会话（可能来自之前中断的上传）。请到网页端删除这个\"上传中\"的文件条目，或稍候几分钟让会话过期后重试。".into(),
        ),
        Err(e) => Err(map_api_err(e)),
    }
}

fn upload_parts_parallel(
    path: &Path,
    size: i64,
    chunk: i64,
    urls: &[String],
    agent: &ureq::Agent,
    policy_type: &str,
) -> Result<Vec<Option<String>>, String> {
    let progress_slots = Arc::new(Mutex::new(vec![0u64; urls.len()]));
    let mut etags = vec![None; urls.len()];

    for start in (0..urls.len()).step_by(MAX_PARALLEL_PARTS) {
        let end = (start + MAX_PARALLEL_PARTS).min(urls.len());
        let batch = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(end - start);
            for index in start..end {
                let url = &urls[index];
                let progress_slots = Arc::clone(&progress_slots);
                handles.push(scope.spawn(move || -> Result<(usize, Option<String>), String> {
                    let mut file = std::fs::File::open(path)
                        .map_err(|e| format!("打开分片文件失败: {e}"))?;
                    let offset = index as i64 * chunk;
                    let len = chunk.min(size - offset).max(0) as u64;
                    file.seek(std::io::SeekFrom::Start(offset as u64))
                        .map_err(|e| format!("定位读取位置失败: {e}"))?;

                    let report: Arc<dyn Fn(u64) + Send + Sync> = {
                        let progress_slots = Arc::clone(&progress_slots);
                        Arc::new(move |part_sent| {
                            let total = {
                                let mut slots = progress_slots.lock().unwrap();
                                slots[index] = part_sent;
                                slots.iter().sum()
                            };
                            crate::progress::set_sent(total);
                        })
                    };
                    let started = std::time::Instant::now();
                    let etag = api::put_part_with_progress(
                        agent, &mut file, len, url, report,
                    )
                    .map_err(|e| format!("上传第 {} 块失败: {e}", index + 1))?;
                    util::log(&format!(
                        "upload part done: part={}/{} bytes={len} elapsed_ms={} parallel={MAX_PARALLEL_PARTS}",
                        index + 1,
                        urls.len(),
                        started.elapsed().as_millis(),
                    ));
                    if policy_type == "s3" && etag.is_none() {
                        return Err(format!("上传第 {} 块: S3 未返回 ETag", index + 1));
                    }
                    Ok((index, etag))
                }));
            }

            let mut results = Vec::with_capacity(handles.len());
            for handle in handles {
                let result = handle
                    .join()
                    .map_err(|_| "上传线程异常终止".to_string())??;
                results.push(result);
            }
            Ok::<_, String>(results)
        })?;

        for (index, etag) in batch {
            etags[index] = etag;
        }
    }

    Ok(etags)
}

fn upload_one(cfg: &Config, key: &str, path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取文件失败: {e}"))?;
    if !meta.is_file() {
        return Err("不是常规文件".into());
    }
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let mime = util::guess_mime(path);
    let uri = format!("cloudreve://my/{}", util::percent_encode_path(&name));

    let session = open_session(cfg, key, &uri, size, mtime, &mime)?;

    let policy = session
        .storage_policy
        .as_ref()
        .ok_or("上传会话缺少存储策略信息")?;
    let policy_type = policy.policy_type.to_lowercase();
    if policy_type != "oss" && policy_type != "s3" {
        return Err(format!("不支持的存储策略类型: {}", policy.policy_type));
    }
    if policy.relay {
        return Err("存储策略为中转模式，客户端仅支持直传".into());
    }
    if session.complete_url.is_empty() || session.upload_urls.is_empty() {
        return Err("上传会话缺少直传地址".into());
    }
    if policy_type == "s3" && session.callback_secret.is_empty() {
        return Err("S3 会话缺少回调密钥".into());
    }

    let chunk = if session.chunk_size > 0 {
        session.chunk_size
    } else {
        size.max(1)
    };
    let expected = if size == 0 {
        1
    } else {
        ((size - 1) / chunk + 1) as usize
    };
    if session.upload_urls.len() != expected {
        return Err(format!(
            "分块数量不匹配（预期 {expected}，实际 {}）",
            session.upload_urls.len()
        ));
    }

    // Keep the pool alive across all parts, retries, and completion.
    let agent = api::transfer_agent();
    util::log(&format!(
        "upload start: bytes={size} chunk_bytes={chunk} parts={expected} policy={policy_type} parallel={MAX_PARALLEL_PARTS}",
    ));
    let run = || -> Result<(), String> {
        let etags = upload_parts_parallel(
            path,
            size,
            chunk,
            &session.upload_urls,
            &agent,
            &policy_type,
        )?;
        let parts: Vec<(u32, String)> = etags
            .into_iter()
            .enumerate()
            .filter_map(|(index, etag)| etag.map(|etag| (index as u32 + 1, etag)))
            .collect();
        if policy_type == "s3" && parts.len() != expected {
            return Err("S3 分片 ETag 数量不完整".into());
        }
        if policy_type == "oss" {
            api::complete_oss(&agent, &session.complete_url)
                .map_err(|e| format!("完成 OSS 上传失败: {e}"))?;
        } else {
            api::complete_s3(&agent, &session.complete_url, &parts)
                .map_err(|e| format!("完成 S3 上传失败: {e}"))?;
            api::s3_callback(&cfg.server, &session.session_id, &session.callback_secret)
                .map_err(|e| format!("S3 回调失败: {}", e.msg))?;
        }
        Ok(())
    };

    if let Err(e) = run() {
        api::cancel_upload_session(&cfg.server, key, &session);
        return Err(e);
    }

    api::create_share(
        &cfg.server,
        key,
        &session.uri,
        cfg.expire_days,
        cfg.downloads,
        cfg.private,
    )
    .map_err(|e| format!("创建分享失败: {}", map_api_err(e)))
}

/// Worker thread body: consumes batches of file paths and reports progress
/// back to the main window. Reads the global CONFIG each batch so settings
/// changes apply to the next upload without a restart.
pub fn worker_static(hwnd: HWND, rx: Receiver<Vec<String>>) {
    while let Ok(files) = rx.recv() {
        let _batch = BatchGuard;
        util::log(&format!("worker: received {} file(s)", files.len()));
        let (server, key, expire, downloads, private) = {
            let c = crate::CONFIG.read().unwrap();
            (
                c.server.clone(),
                crate::config::decrypt_key(&c.api_key_enc),
                c.expire_days,
                c.downloads,
                c.private,
            )
        };
        let work_cfg = Config {
            server,
            api_key_enc: String::new(),
            expire_days: expire,
            downloads,
            private,
            auto_update: true,
        };

        if key.is_empty() {
            util::log("worker: no api key configured, skipping upload");
            post_str(
                hwnd,
                WM_APP_NEED_SETUP,
                0,
                "尚未配置 API Key：请用网页端的配对码完成连接。".into(),
            );
            continue;
        }

        let mut outcome = UploadOutcome {
            ok: vec![],
            failed: vec![],
        };
        let mut cancelled = false;
        unsafe {
            let _ = PostMessageW(
                Some(hwnd),
                crate::progress::WM_PROG_BEGIN,
                WPARAM(0),
                LPARAM(files.len() as isize),
            );
        }
        for (i, f) in files.iter().enumerate() {
            let name = Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| f.clone());
            let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            post_str(
                hwnd,
                WM_APP_UPLOAD_STATE,
                0,
                format!("正在上传 {}/{}：{}", i + 1, files.len(), name),
            );
            let payload =
                Box::into_raw(Box::new((i + 1, files.len(), size, name.clone()))) as isize;
            unsafe {
                let _ = PostMessageW(
                    Some(hwnd),
                    crate::progress::WM_PROG_FILE,
                    WPARAM(0),
                    LPARAM(payload),
                );
            }
            if crate::progress::cancel_requested() {
                cancelled = true;
                break;
            }
            match upload_one(&work_cfg, &key, Path::new(f)) {
                Ok(link) => outcome.ok.push(link),
                Err(e) => {
                    util::log(&format!("upload failed for {f}: {e}"));
                    if crate::progress::cancel_requested() || e.contains("已取消") {
                        cancelled = true;
                        break;
                    }
                    outcome.failed.push((name, e));
                }
            }
        }

        if cancelled && outcome.ok.is_empty() {
            unsafe {
                let _ = PostMessageW(
                    Some(hwnd),
                    crate::progress::WM_PROG_CANCELLED,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            post_str(hwnd, WM_APP_UPLOAD_DONE, 1, "上传已取消".into());
            continue;
        }

        if outcome.ok.is_empty() {
            let msg = outcome
                .failed
                .first()
                .map(|(n, e)| format!("{n}：{e}"))
                .unwrap_or_else(|| "没有文件被上传".into());
            unsafe {
                let _ = PostMessageW(
                    Some(hwnd),
                    crate::progress::WM_PROG_DONE,
                    WPARAM(1),
                    LPARAM(Box::into_raw(Box::new(msg.clone())) as isize),
                );
            }
            post_str(hwnd, WM_APP_UPLOAD_DONE, 1, msg);
        } else {
            let text = if outcome.ok.len() == 1 && outcome.failed.is_empty() && !cancelled {
                let name = Path::new(&files[0])
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| files[0].clone());
                format!("上传完成：{name}\n分享链接已复制到剪贴板")
            } else {
                let mut t = format!(
                    "{} 个文件上传成功，{} 条链接已复制",
                    outcome.ok.len(),
                    outcome.ok.len()
                );
                for (n, e) in &outcome.failed {
                    t.push_str(&format!("\n失败 {n}：{e}"));
                }
                if cancelled {
                    t.push_str("\n（已取消剩余文件）");
                }
                t
            };
            unsafe {
                let _ = PostMessageW(
                    Some(hwnd),
                    crate::progress::WM_PROG_DONE,
                    WPARAM(0),
                    LPARAM(Box::into_raw(Box::new(text.clone())) as isize),
                );
            }
            post_str(hwnd, WM_APP_UPLOAD_DONE, 0, text);
            let links = outcome.ok.join("\r\n");
            post_str(hwnd, WM_APP_UPLOAD_LINKS, 0, links);
        }
    }
}
