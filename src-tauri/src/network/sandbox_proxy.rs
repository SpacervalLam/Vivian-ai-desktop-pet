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

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::utils::sandbox_policy::{domain_allowed, host_of_authority};

/// 单连接读写超时：沙箱命令本身有超时兜底，这里只防卡死的半开连接
const IO_TIMEOUT: Duration = Duration::from_secs(120);

/// 已启动的沙箱代理
pub struct SandboxProxy {
    /// 实际监听地址。**端口可能被系统重新分配过**，注入环境变量要用这个，
    /// 不能用配置里的原始值。
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
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
    }
}

/// 启动沙箱代理
///
/// 只负责监听与过滤，不判断"该不该启用"——那是调用方按沙箱配置决定的事，
/// 这里保持无配置依赖，便于单独编译验证。
///
/// `port = 0` 时由系统分配空闲端口，实际端口从返回值的 `addr` 读回。
pub fn start(host: &str, port: u16, allow_domains: &[String]) -> Result<SandboxProxy, String> {
    let host = if host.trim().is_empty() { "127.0.0.1" } else { host.trim() };
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
    thread::spawn(move || accept_loop(listener, allow, stop_flag));

    tracing::info!(
        "[sandbox] 代理已启动 {}，白名单 {} 条（留空 = 拒绝全部）",
        actual,
        allow_domains.len()
    );
    Ok(SandboxProxy { addr: actual, stop })
}

fn accept_loop(listener: TcpListener, allow: Vec<String>, stop: Arc<AtomicBool>) {
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                let allow = allow.clone();
                thread::spawn(move || {
                    if let Err(e) = handle(stream, &allow) {
                        tracing::debug!("[sandbox] 代理连接结束: {e}");
                    }
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                tracing::warn!("[sandbox] 代理 accept 失败: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// 处理单个代理连接
fn handle(mut client: TcpStream, allow: &[String]) -> Result<(), String> {
    let mut reader = BufReader::new(client.try_clone().map_err(|e| e.to_string())?);

    // 请求行：`METHOD target HTTP/1.1`
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).map_err(|e| e.to_string())? == 0 {
        return Err("空请求".to_string());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    if method.is_empty() || target.is_empty() {
        return Err(format!("请求行无法解析: {}", request_line.trim()));
    }

    // 目标形式：CONNECT 是 host:port，其余是绝对 URI
    let authority = if method.eq_ignore_ascii_case("CONNECT") {
        target.clone()
    } else {
        strip_scheme(&target).to_string()
    };
    let host = host_of_authority(&authority).to_string();

    if !domain_allowed(&host, allow) {
        tracing::warn!("[sandbox] 代理拒绝 {}（不在白名单）", host);
        let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        let _ = client.flush();
        return Err(format!("域名不在白名单: {host}"));
    }

    let _ = client.set_read_timeout(Some(IO_TIMEOUT));
    let _ = client.set_write_timeout(Some(IO_TIMEOUT));

    if method.eq_ignore_ascii_case("CONNECT") {
        let upstream =
            TcpStream::connect(&authority).map_err(|e| format!("连接 {authority} 失败: {e}"))?;
        let _ = upstream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = upstream.set_write_timeout(Some(IO_TIMEOUT));
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(|e| e.to_string())?;
        client.flush().map_err(|e| e.to_string())?;
        tracing::debug!("[sandbox] 代理放行隧道 {}", host);
        pipe(client, upstream);
        Ok(())
    } else {
        // 明文 HTTP：把绝对 URI 改写成相对路径，再原样转发其余头部与正文
        let path = path_of_uri(&target);
        let mut upstream =
            TcpStream::connect(&authority).map_err(|e| format!("连接 {authority} 失败: {e}"))?;
        let _ = upstream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = upstream.set_write_timeout(Some(IO_TIMEOUT));

        let mut out = format!("{method} {path} HTTP/1.1\r\n").into_bytes();
        loop {
            let mut line = Vec::new();
            let n = reader
                .read_until(b'\n', &mut line)
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            // 空行 = 头部结束
            if line == b"\r\n" || line == b"\n" {
                out.extend_from_slice(b"\r\n");
                break;
            }
            let s = String::from_utf8_lossy(&line);
            let lower = s.to_lowercase();
            // 代理专用头不该透传给源站
            if lower.starts_with("proxy-") || lower.starts_with("host:") {
                if lower.starts_with("host:") {
                    out.extend_from_slice(format!("Host: {authority}\r\n").as_bytes());
                }
                continue;
            }
            out.extend_from_slice(&line);
        }
        upstream.write_all(&out).map_err(|e| e.to_string())?;
        upstream.flush().map_err(|e| e.to_string())?;

        // 剩余正文（若有）
        let mut rest = Vec::new();
        let _ = reader.read_to_end(&mut rest);
        if !rest.is_empty() {
            // 正文已在上面的 read_to_end 里从 reader 读出，但 reader 与 client
            // 共享同一 fd，缓冲里未消费的部分会随 try_clone 一起关闭——显式转发
            upstream.write_all(&rest).map_err(|e| e.to_string())?;
            upstream.flush().map_err(|e| e.to_string())?;
        }
        tracing::debug!("[sandbox] 代理放行 {} {}", method, host);
        pipe(client, upstream);
        Ok(())
    }
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
    let rest = uri
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(uri);
    rest.split('/').next().unwrap_or(rest)
}

/// `http://host:port/path?q` → `/path?q`
fn path_of_uri(uri: &str) -> String {
    let rest = uri.split_once("://").map(|(_, r)| r).unwrap_or(uri);
    match rest.find('/') {
        Some(i) => rest[i..].to_string(),
        None => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_and_path_parsing() {
        assert_eq!(strip_scheme("http://example.com:80/a/b?c=1"), "example.com:80");
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
}
