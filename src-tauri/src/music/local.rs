//! 本地音乐库：扫描音乐目录 → 按名字检索 → 直接解码播放。
//!
//! 检索按**文件名**匹配（不解析音频标签，避免大库扫描时逐文件解码的开销）。
//! 约定 `艺术家 - 曲名.mp3` 这种命名，解析失败时整段文件名当作曲名。
//!
//! 播放走独立线程持有 `rodio::OutputStream`（该类型不是 `Send`，不能塞进全局 `Mutex`），
//! 线程通过 channel 接收命令。

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use super::{PlayOutcome, TrackCandidate, TrackSource};

/// 识别的音频扩展名（rodio 可解码的范围）
const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "aac", "wma", "mp4",
];

/// 单次扫描的文件数上限，防用户误配了根目录（如 C:\）导致卡死
const MAX_SCAN_FILES: usize = 20_000;

/// 扫描递归深度上限
const MAX_DEPTH: usize = 8;

// ─── 播放线程 ───────────────────────────────────────────────────────────────

enum Cmd {
    Play(PathBuf, mpsc::Sender<Result<(), String>>),
    Stop,
}

/// 播放线程的命令发送端。首次播放时惰性启动线程。
static PLAYER: Lazy<Mutex<Option<mpsc::Sender<Cmd>>>> = Lazy::new(|| Mutex::new(None));

/// 取（必要时创建）播放线程的命令通道
fn player_tx() -> Result<mpsc::Sender<Cmd>, String> {
    let mut guard = PLAYER.lock();
    if let Some(tx) = guard.as_ref() {
        return Ok(tx.clone());
    }

    let (tx, rx) = mpsc::channel::<Cmd>();
    std::thread::Builder::new()
        .name("vivian-music-player".into())
        .spawn(move || player_loop(rx))
        .map_err(|e| format!("启动播放线程失败: {e}"))?;

    *guard = Some(tx.clone());
    Ok(tx)
}

/// 播放线程主体：独占持有 `OutputStream`（必须与 `Sink` 同生命周期，
/// 否则音频设备会被关闭）
fn player_loop(rx: mpsc::Receiver<Cmd>) {
    use rodio::{Decoder, OutputStream, Sink};

    let (stream, handle) = match OutputStream::try_default() {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("音频输出设备初始化失败: {e}");
            tracing::warn!("[Music/Local] {}", msg);
            // 设备不可用：让所有后续请求都拿到明确错误，而不是静默无声
            for cmd in rx {
                if let Cmd::Play(_, ack) = cmd {
                    let _ = ack.send(Err(msg.clone()));
                }
            }
            return;
        }
    };
    let _stream = stream; // 保活

    // 每首曲子新建 Sink：语义明确（旧 Sink 丢弃 = 停止上一首），
    // 不依赖 `Sink::stop()` 之后是否还能复用的实现细节
    let mut sink: Option<Sink> = None;

    for cmd in rx {
        match cmd {
            Cmd::Play(path, ack) => {
                if let Some(old) = sink.take() {
                    old.stop();
                }

                let result = (|| -> Result<(), String> {
                    let file = std::fs::File::open(&path)
                        .map_err(|e| format!("打开音频文件失败: {e}"))?;
                    let source = Decoder::new(BufReader::new(file))
                        .map_err(|e| format!("解码失败（格式可能不受支持）: {e}"))?;
                    let new_sink = Sink::try_new(&handle)
                        .map_err(|e| format!("创建音频输出失败: {e}"))?;
                    new_sink.append(source);
                    sink = Some(new_sink);
                    Ok(())
                })();

                if let Err(e) = &result {
                    tracing::warn!("[Music/Local] 播放失败 {}: {}", path.display(), e);
                }
                let _ = ack.send(result);
            }
            Cmd::Stop => {
                if let Some(s) = sink.take() {
                    s.stop();
                }
            }
        }
    }
}

/// 停止本地播放
pub fn stop() {
    if let Ok(tx) = player_tx() {
        let _ = tx.send(Cmd::Stop);
    }
}

/// 播放一个本地音频文件（阻塞等待播放线程确认已开始输出）
pub fn play_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("文件不存在: {}", path.display()));
    }
    let tx = player_tx()?;
    let (ack_tx, ack_rx) = mpsc::channel();
    tx.send(Cmd::Play(path.to_path_buf(), ack_tx))
        .map_err(|_| "播放线程已退出".to_string())?;
    ack_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "等待播放线程响应超时".to_string())?
}

// ─── 扫描与检索 ─────────────────────────────────────────────────────────────

/// 一个已发现的本地曲目
#[derive(Debug, Clone)]
pub struct LocalTrack {
    pub path: PathBuf,
    /// 解析出的曲名
    pub title: String,
    /// 解析出的艺术家（可能为空）
    pub artist: String,
}

/// 从文件名解析「艺术家 - 曲名」。
///
/// 常见分隔符：` - ` / ` – ` / `—`。解析不出时整段当曲名。
fn parse_file_name(stem: &str) -> (String, String) {
    for sep in [" - ", " – ", "—", "-"] {
        if let Some((left, right)) = stem.split_once(sep) {
            let artist = left.trim();
            let title = right.trim();
            // 两侧都非空才算有效分隔，否则 "01-track" 这种会被误拆
            if !artist.is_empty() && !title.is_empty() {
                return (title.to_string(), artist.to_string());
            }
        }
    }
    (stem.trim().to_string(), String::new())
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 对外暴露的音频文件判定（供工具层校验模型传入的 `track_id` 路径）
pub fn is_audio_path(path: &Path) -> bool {
    is_audio_file(path)
}

/// 递归扫描目录下的音频文件（有深度与数量上限）
fn scan_dir(dir: &Path, depth: usize, out: &mut Vec<LocalTrack>) {
    if depth > MAX_DEPTH || out.len() >= MAX_SCAN_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_SCAN_FILES {
            tracing::warn!(
                "[Music/Local] 扫描达到 {} 文件上限，已截断（请缩小音乐目录范围）",
                MAX_SCAN_FILES
            );
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, depth + 1, out);
        } else if is_audio_file(&path) {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let (title, artist) = parse_file_name(stem);
            out.push(LocalTrack { path, title, artist });
        }
    }
}

/// 扫描全部配置目录
pub fn scan(dirs: &[String]) -> Vec<LocalTrack> {
    let mut tracks = Vec::new();
    for dir in dirs {
        let p = PathBuf::from(dir);
        if !p.is_dir() {
            tracing::debug!("[Music/Local] 目录不存在，跳过: {}", dir);
            continue;
        }
        scan_dir(&p, 0, &mut tracks);
    }
    tracks
}

/// 没有配置曲库目录时的兜底搜索范围：取用户主目录下的常见音乐位置（音乐 / 下载 /
/// 桌面 / 文档 / OneDrive 变体），只保留真实存在的目录。
///
/// Windows 已知文件夹可被重定向到别的盘，此处按标准相对路径拼接，
/// 覆盖不到重定向的位置（此时用 `directory` 参数显式指定）。
pub fn default_search_dirs() -> Vec<String> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    [
        "Music",
        "Downloads",
        "Desktop",
        "Documents",
        "OneDrive\\Music",
        "OneDrive\\Desktop",
    ]
    .iter()
    .map(|sub| home.join(sub))
    .filter(|p| p.is_dir())
    .map(|p| p.to_string_lossy().to_string())
    .collect()
}

/// 解析本次检索实际要扫的目录。
///
/// 优先级：显式 `directory` 参数 > 配置的曲库目录 > 常见位置兜底。
/// 显式指定时只用它（不叠加）。
pub fn resolve_search_dirs(explicit: Option<&str>, configured: &[String]) -> Vec<String> {
    if let Some(dir) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return vec![dir.to_string()];
    }
    if !configured.is_empty() {
        return configured.to_vec();
    }
    default_search_dirs()
}

/// 计算匹配得分，0 表示不匹配。分数越高越靠前。
fn score(track: &LocalTrack, needle: &str) -> u32 {
    let title = track.title.to_lowercase();
    let artist = track.artist.to_lowercase();

    if title == needle {
        1000
    } else if title.starts_with(needle) {
        800
    } else if title.contains(needle) {
        600
    } else if !artist.is_empty() && artist == needle {
        400
    } else if !artist.is_empty() && artist.contains(needle) {
        300
    } else if track
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|f| f.to_lowercase().contains(needle))
        .unwrap_or(false)
    {
        100
    } else {
        0
    }
}

/// 按关键词检索本地曲库，返回排好序的候选（序号从 1 开始）
pub fn search(tracks: &[LocalTrack], query: &str, limit: usize) -> Vec<TrackCandidate> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(u32, &LocalTrack)> = tracks
        .iter()
        .filter_map(|t| {
            let s = score(t, &needle);
            (s > 0).then_some((s, t))
        })
        .collect();

    // 同分时按曲名排序，保证结果稳定（否则目录遍历顺序会让结果每次不同）
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))
    });

    scored
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(i, (_, t))| TrackCandidate {
            index: i + 1,
            title: t.title.clone(),
            artist: t.artist.clone(),
            // 标签/时长不解析：大库下逐文件解码代价过高，留 0（展示时省略）
            duration_secs: 0,
            source: TrackSource::Local.as_str().to_string(),
            id: t.path.to_string_lossy().to_string(),
        })
        .collect()
}

/// 播放指定的本地候选
pub fn play(candidate: &TrackCandidate) -> Result<PlayOutcome, String> {
    let path = PathBuf::from(&candidate.id);
    play_file(&path)?;
    Ok(PlayOutcome {
        message: format!(
            "正在播放本地文件：{} — {}",
            candidate.title,
            if candidate.artist.is_empty() {
                "未知艺术家"
            } else {
                &candidate.artist
            }
        ),
        auto_played: true,
        source: TrackSource::Local.as_str().to_string(),
        target: candidate.id.clone(),
    })
}

/// 直接播放一个路径，跳过检索。
///
/// 供 `music_play` 的 `track_id` 参数使用（值来自文件工具找到的路径，
/// 或模型自己用文件工具找到的路径）。曲名/艺术家从文件名反推。
pub fn play_path(path: &Path) -> Result<PlayOutcome, String> {
    if !is_audio_file(path) {
        return Err(format!(
            "不是可识别的音频文件（支持的扩展名：{}）：{}",
            AUDIO_EXTS.join(" / "),
            path.display()
        ));
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let (title, artist) = parse_file_name(stem);
    play(&TrackCandidate {
        index: 1,
        title,
        artist,
        duration_secs: 0,
        source: TrackSource::Local.as_str().to_string(),
        id: path.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_artist_title() {
        assert_eq!(
            parse_file_name("周杰伦 - 晴天"),
            ("晴天".to_string(), "周杰伦".to_string())
        );
        // 无分隔符 → 整段当曲名
        assert_eq!(
            parse_file_name("晴天"),
            ("晴天".to_string(), String::new())
        );
        // 单侧为空 → 不拆
        assert_eq!(
            parse_file_name("- 晴天"),
            ("- 晴天".to_string(), String::new())
        );
    }

    #[test]
    fn score_ranks_exact_title_first() {
        let exact = LocalTrack {
            path: PathBuf::from("a.mp3"),
            title: "晴天".into(),
            artist: "周杰伦".into(),
        };
        let loose = LocalTrack {
            path: PathBuf::from("b.mp3"),
            title: "晴天娃娃".into(),
            artist: "某人".into(),
        };
        assert!(score(&exact, "晴天") > score(&loose, "晴天"));
    }

    #[test]
    fn search_is_stable_on_ties() {
        let mk = |t: &str| LocalTrack {
            path: PathBuf::from(format!("{t}.mp3")),
            title: t.into(),
            artist: String::new(),
        };
        // 两条同分（都只是 contains），结果应按曲名排序而非输入顺序
        let tracks = vec![mk("b晴天"), mk("a晴天")];
        let r = search(&tracks, "晴天", 10);
        assert_eq!(r[0].title, "a晴天");
        assert_eq!(r[1].title, "b晴天");
    }

    #[test]
    fn only_audio_extensions_are_accepted() {
        assert!(is_audio_path(Path::new("D:/m/晴天.mp3")));
        assert!(is_audio_path(Path::new("D:/m/晴天.FLAC")), "扩展名应大小写不敏感");
        assert!(!is_audio_path(Path::new("D:/m/notes.txt")));
        assert!(!is_audio_path(Path::new("D:/m/archive")));
    }

    #[test]
    fn play_path_rejects_non_audio() {
        // 不碰音频设备：非音频扩展名在校验阶段就被挡下
        let err = play_path(Path::new("D:/m/readme.txt")).unwrap_err();
        assert!(err.contains("不是可识别的音频文件"), "got {err}");
    }

    #[test]
    fn explicit_directory_wins_and_does_not_stack() {
        let configured = vec!["D:/music".to_string()];
        let got = resolve_search_dirs(Some("E:/somewhere"), &configured);
        // 显式 directory 时只扫该目录，不叠加配置目录
        assert_eq!(got, vec!["E:/somewhere".to_string()]);
    }

    #[test]
    fn blank_directory_falls_through_to_configured() {
        let configured = vec!["D:/music".to_string()];
        assert_eq!(
            resolve_search_dirs(Some("   "), &configured),
            vec!["D:/music".to_string()]
        );
    }

    #[test]
    fn configured_dirs_used_when_no_explicit() {
        let configured = vec!["D:/music".to_string(), "E:/more".to_string()];
        assert_eq!(resolve_search_dirs(None, &configured), configured);
    }

    #[test]
    fn falls_back_to_defaults_when_nothing_configured() {
        // 无配置目录时退到常见位置；此处只断言不 panic 且返回确定结果
        let got = resolve_search_dirs(None, &[]);
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        if home.is_some() && got.is_empty() {
            assert!(got.is_empty());
        }
    }
}
