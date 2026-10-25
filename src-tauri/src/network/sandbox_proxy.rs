//! 沙箱本地代理 —— 按域名白名单放行的正向代理
//!
//! 为什么需要这一层：受限令牌 + 低完整性这条路线上，Windows 没有轻量手段做
//! 按进程的网络过滤（Job Object 只能限速不能断网，WFP 要驱动级组件）。可行的
//! 做法是把沙箱子进程的流量引到一个本地代理，由代理按域名决定放行。
//!
//! **这是协作式约束，不是内核级边界。** 子进程若不读 `HTTP_PROXY` 环境变量、
//! 直接建 socket 出去，代理拦不住。它覆盖的是 curl / PowerShell / .NET / npm /
//! git 这类按约定行事的程序——它们占了实际调用的绝大多数，但"占绝大多数"不
//! 等于"挡得住"。真要硬边界，得换成独立账户 + 按账户的防火墙规则（那是另一套
//! 架构，代价是账户与凭据管理）。
//!
//! 端口不写死：配置为 0 时由系统分配空闲端口，启动后从 `addr` 读回实际值。
//! 硬编码端口会在启动时与本机其他服务撞车，而撞车后无从恢复。

#[cfg(test)]
use std::io::Read;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::utils::sandbox_policy::{domain_allowed, host_of_authority};

/// 单连接读写超时：沙箱命令本身有超时兜底，这里只防卡死的半开连接
const IO_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_CONNECTIONS: usize = 64;

/// 已启动的沙箱代理
pub struct SandboxProxy {
    /// 实际监听地址。**端口可能被系统重新分配过**，注入环境变量要用这个，
    /// 不能用配置里的原始值。
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SandboxProxy {
    /// 注入子进程用的 `host:port`
    pub fn authority(&self) -> String {
        self.addr.to_string()
    }
}

impl Drop for SandboxProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

/// 启动沙箱代理
///
/// 只负责监听与过滤，不判断"该不该启用"——那是调用方按沙箱配置决定的事，
/// 这里保持无配置依赖，便于单独编译验证。
///
/// `port = 0` 时由系统分配空闲端口，实际端口从返回值的 `addr` 读回。
pub fn start(host: &str, port: u16, allow_domains: &[String]) -> Result<SandboxProxy, String> {
    let host = if host.trim().is_empty() {
        "127.0.0.1"
    } else {
        host.trim()
    };
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| format!("沙箱代理地址无效（{host}:{port}）: {e}"))?;

    let listener = TcpListener::bind(addr).map_err(|e| {
        format!("沙箱代理监听 {addr} 失败: {e}；把端口改成 0 可由系统自动分配空闲端口")
    })?;
    // 非阻塞 + 轮询：进程退出时能被 stop 标志打断，不必等下一个连接进来
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("设置非阻塞监听失败: {e}"))?;
    let actual = listener
        .local_addr()
        .map_err(|e| format!("读取代理监听地址失败: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let allow = allow_domains.to_vec();
    let stop_flag = Arc::clone(&stop);
    let worker = thread::spawn(move || accept_loop(listener, allow, stop_flag));

    tracing::info!(
        "[sandbox] 代理已启动 {}，白名单 {} 条（留空 = 拒绝全部）",
        actual,
        allow_domains.len()
    );
    Ok(SandboxProxy {
        addr: actual,
        stop,
        worker: Some(worker),
    })
}

fn accept_loop(listener: TcpListener, allow: Vec<String>, stop: Arc<AtomicBool>) {
    let allow = Arc::new(allow);
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((mut stream, _peer)) => {
                if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    let _ = stream.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
                    );
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let active = Arc::clone(&active);
                let allow = Arc::clone(&allow);
                thread::spawn(move || {
                    if let Err(e) = handle(stream, &allow) {
                        tracing::debug!("[sandbox] 代理连接结束: {e}");
                    }
                    active.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::park_timeout(Duration::from_millis(100));
            }
            Err(e) => {
                tracing::warn!("[sandbox] 代理 accept 失败: {e}");
                thread::park_timeout(Duration::from_millis(100));
            }
        }
    }
}

/// 处理单个代理连接
fn read_header_line(
    reader: &mut BufReader<TcpStream>,
    remaining: &mut usize,
) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    std::io::Read::take(&mut *reader, remaining.saturating_add(1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|e| e.to_string())?;
    if line.is_empty() || line.len() > *remaining || !line.ends_with(b"\n") {
        return Err("代理请求头不完整或超过 32 KiB".into());
    }
    *remaining -= line.len();
    Ok(line)
}

fn handle(mut client: TcpStream, allow: &[String]) -> Result<(), String> {
    client
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    client
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(client.try_clone().map_err(|e| e.to_string())?);
    let mut remaining = MAX_HEADER_BYTES;
    let request = read_header_line(&mut reader, &mut remaining)?;
    let request = std::str::from_utf8(&request).map_err(|_| "无效代理请求行")?;
    let parts: Vec<_> = request.split_whitespace().collect();
    if parts.len() != 3 || !parts[2].starts_with("HTTP/1.") {
        return Err("无效代理请求行".into());
    }
    let (method, target) = (parts[0], parts[1]);
    let tunnel = method.eq_ignore_ascii_case("CONNECT");
    if !tunnel && !target.starts_with("http://") {
        return Err("明文代理仅支持 http:// 绝对 URI，HTTPS 请使用 CONNECT".into());
    }
    let authority = if tunnel { target } else { strip_scheme(target) };
    if authority.is_empty() || authority.contains(['@', '\\', '/', '?', '#']) {
        return Err("无效代理主机".into());
    }
    let host = host_of_authority(authority);
    if host.is_empty() || !domain_allowed(host, allow) {
        client
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .map_err(|e| e.to_string())?;
        return Err(format!("域名不在白名单: {host}"));
    }
    let mut headers = Vec::new();
    loop {
        let line = read_header_line(&mut reader, &mut remaining)?;
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let text = std::str::from_utf8(&line).map_err(|_| "无效请求头")?;
        let (name, _) = text.split_once(':').ok_or("无效请求头")?;
        if !name.to_ascii_lowercase().starts_with("proxy-")
            && !name.eq_ignore_ascii_case("host")
            && !name.eq_ignore_ascii_case("connection")
        {
            headers.extend_from_slice(&line);
        }
    }
    let explicit_port = if authority.starts_with('[') {
        authority
            .split_once(']')
            .is_some_and(|(_, rest)| rest.starts_with(':'))
    } else {
        authority.contains(':')
    };
    if tunnel && !explicit_port {
        return Err("CONNECT 必须指定端口".into());
    }
    let address = if explicit_port {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    use std::net::ToSocketAddrs;
    let mut upstream = address
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .take(8)
        .find_map(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(10)).ok())
        .ok_or_else(|| format!("连接 {address} 失败"))?;
    upstream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    upstream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    if tunnel {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(|e| e.to_string())?;
        client.flush().map_err(|e| e.to_string())?;
    } else {
        let request = format!(
            "{method} {} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n",
            path_of_uri(target)
        );
        upstream
            .write_all(request.as_bytes())
            .map_err(|e| e.to_string())?;
        upstream.write_all(&headers).map_err(|e| e.to_string())?;
        upstream.write_all(b"\r\n").map_err(|e| e.to_string())?;
    }
    // Forward bytes already buffered with the headers, then stream both ways.
    // Waiting for EOF here deadlocks normal GETs and drops early CONNECT payloads.
    upstream
        .write_all(reader.buffer())
        .map_err(|e| e.to_string())?;
    upstream.flush().map_err(|e| e.to_string())?;
    drop(reader);
    pipe(client, upstream);
    Ok(())
}
/// 双向转发（两个方向各一个线程，任一方结束就 shutdown 唤醒对端）
fn pipe(a: TcpStream, b: TcpStream) {
    let mut a2 = match a.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut b2 = match b.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut a1 = a;
    let mut b1 = b;

    let t1 = thread::spawn(move || {
        let _ = std::io::copy(&mut a1, &mut b2);
        let _ = b2.shutdown(std::net::Shutdown::Write);
    });
    let t2 = thread::spawn(move || {
        let _ = std::io::copy(&mut b1, &mut a2);
        let _ = a2.shutdown(std::net::Shutdown::Write);
    });
    let _ = t1.join();
    let _ = t2.join();
}

/// `http://host:port/path` → `host:port`
fn strip_scheme(uri: &str) -> &str {
    let rest = uri.split_once("://").map(|(_, r)| r).unwrap_or(uri);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// `http://host:port/path?q` → `/path?q`
fn path_of_uri(uri: &str) -> String {
    let rest = uri.split_once("://").map(|(_, r)| r).unwrap_or(uri);
    match rest.find(['/', '?', '#']) {
        Some(i) if rest.as_bytes()[i] == b'/' => {
            rest[i..].split('#').next().unwrap_or("/").to_string()
        }
        Some(i) => format!("/{}", rest[i..].split('#').next().unwrap_or("")),
        None => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_and_path_parsing() {
        assert_eq!(
            strip_scheme("http://example.com:80/a/b?c=1"),
            "example.com:80"
        );
        assert_eq!(path_of_uri("http://example.com:80/a/b?c=1"), "/a/b?c=1");
        assert_eq!(path_of_uri("http://example.com"), "/");
    }

    #[test]
    fn port_zero_gets_assigned_by_os() {
        let p = start("127.0.0.1", 0, &["example.com".to_string()]).expect("端口 0 应由系统分配");
        assert_ne!(p.addr.port(), 0, "实际端口必须读回真实值");
        assert_eq!(p.authority(), p.addr.to_string());
    }

    /// 白名单外的域名必须拿到 403——这是整层的意义所在，不联网也能验证
    #[test]
    fn non_allowlisted_domain_gets_403() {
        let p = start("127.0.0.1", 0, &["example.com".to_string()]).expect("代理应启动");
        let mut c = TcpStream::connect(p.addr).expect("连上本地代理");
        let _ = c.write_all(b"CONNECT evil.com:443 HTTP/1.1\r\nHost: evil.com:443\r\n\r\n");
        let _ = c.flush();
        let mut buf = [0u8; 64];
        let n = c.read(&mut buf).expect("应收到响应");
        let resp = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(resp.starts_with("HTTP/1.1 403"), "实际响应: {resp}");
    }

    #[test]
    fn get_does_not_wait_for_client_eof_and_strips_proxy_credentials() {
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = origin.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (stream, _) = origin.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                headers.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert!(headers.starts_with("GET /?q=1 HTTP/1.1"), "{headers}");
            assert!(!headers.to_ascii_lowercase().contains("proxy-authorization"));
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let proxy = start("127.0.0.1", 0, &["127.0.0.1".into()]).unwrap();
        let mut client = TcpStream::connect(proxy.addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(client, "GET http://{addr}?q=1 HTTP/1.1\r\nHost: invalid\r\nProxy-Authorization: secret\r\n\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.ends_with("ok"), "{response}");
        worker.join().unwrap();
    }

    #[test]
    fn connect_consumes_headers_and_preserves_buffered_tunnel_bytes() {
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = origin.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = origin.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut data = [0u8; 4];
            stream.read_exact(&mut data).unwrap();
            assert_eq!(&data, b"ping");
            stream.write_all(b"pong").unwrap();
        });
        let proxy = start("127.0.0.1", 0, &["127.0.0.1".into()]).unwrap();
        let mut client = TcpStream::connect(proxy.addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            client,
            "CONNECT {addr} HTTP/1.1\r\nHost: {addr}\r\n\r\nping"
        )
        .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("pong"), "{response}");
        worker.join().unwrap();
    }

    #[test]
    fn dropping_a_proxy_releases_its_port_before_returning() {
        let proxy = start("127.0.0.1", 0, &[]).unwrap();
        let addr = proxy.addr;
        drop(proxy);
        assert!(TcpListener::bind(addr).is_ok());
    }
}
