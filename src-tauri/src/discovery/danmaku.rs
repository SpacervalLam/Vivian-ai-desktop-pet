//! B 站弹幕 — 分段 protobuf 解析与时间锚聚合
//!
//! 推荐评估时不灌入全部原文，按需浏览时可分页读取带时间的原文。聚合保留两件
//! 字幕给不了的信息——**哪个时间点观众在集中反应**（密度），以及**那个点上大家
//! 在重复同一句话**（梗）。
//!
//! 所以这里只做「按分钟分桶 → 取密度最高的几分钟 → 每桶带一句代表弹幕」。
//! 产出的摘要只作评估的辅助锚点，不替代字幕/正文，也不当内容事实。
//!
//! 解析对象是 `/x/v2/dm/web/seg.so` 的 protobuf 响应（6 分钟一段）。外层是一串
//! length-delimited 的 `DanmakuElem`；内层只认两个字段——`progress`(2, varint,
//! 毫秒) 与 `content`(7, string)，其余按 wire type 跳过。不引 protobuf 依赖：
//! 这两个字段号与 wire-format 是公开且长期稳定的，只认它们比维护整份 schema
//! 更抗上游加字段。

/// 单条弹幕（只保留时间锚与文本）
#[derive(Debug, Clone, PartialEq)]
pub struct DanmakuElem {
    /// 相对视频开头的毫秒数
    pub progress_ms: u32,
    pub text: String,
}

/// 一分钟的弹幕桶
#[derive(Debug, Clone, PartialEq)]
pub struct MinuteBucket {
    /// 从 1 开始（人类口径，不出现「第 0 分钟」）
    pub minute: u32,
    pub count: usize,
    /// 该分钟的代表弹幕；`repeat` ≥ 2 表示这是被刷屏的那句
    pub top_text: String,
    pub repeat: usize,
}

/// 聚合结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DanmakuDigest {
    pub total: usize,
    /// 实际覆盖的时长（秒）——分段抓取只覆盖前若干分钟，必须如实标注
    pub window_secs: u64,
    /// 密度最高的几分钟，按时间升序
    pub peak: Vec<MinuteBucket>,
}

/// 重复达到这么多条才算「梗」，否则改用该分钟最长的一句
const MIN_REPEAT_FOR_MEME: usize = 2;
/// 一分钟至少这么多条才进高峰榜（1-2 条不构成集中反应）
const MIN_COUNT_PER_PEAK: usize = 3;
/// 高峰榜最多几段
const MAX_PEAK_MINUTES: usize = 3;
/// 代表弹幕截断长度
const MAX_TEXT_CHARS: usize = 32;

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// 纯标点/纯符号的弹幕（「？」「？？？」「……」）没有描述性，不选作代表句
///
/// 这么过滤只影响「选哪一句」，不影响计数——它们仍然是真实的观众反应，
/// 只是没法用一句话说出来。
fn is_meaningful(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// protobuf wire-format 的最小读取器
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// varint（LEB128）；超 64 位视为损坏
    fn read_varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.buf.get(self.pos)?;
            self.pos += 1;
            if shift == 63 && byte & 0xfe != 0 {
                return None;
            }
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }

    /// 取 `len` 字节；越界返回 None（损坏/截断数据不 panic）
    fn read_bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    /// 按 wire type 跳过字段（未用到的字段一律走这里）
    fn skip(&mut self, wire_type: u64) -> Option<()> {
        match wire_type {
            0 => self.read_varint().map(|_| ()),
            1 => self.read_bytes(8).map(|_| ()),
            2 => {
                let len = self.read_varint()?;
                let len = usize::try_from(len).ok()?;
                self.read_bytes(len).map(|_| ())
            }
            5 => self.read_bytes(4).map(|_| ()),
            // 3/4 是已废弃的 group 语义，无法可靠跳过 → 停止解析
            _ => None,
        }
    }
}

/// 解析一条 `DanmakuElem`；没有正文的条目（表情包/纯空白）直接丢弃
fn parse_elem(payload: &[u8]) -> Option<Option<DanmakuElem>> {
    let mut r = Reader::new(payload);
    let mut progress_ms = 0u32;
    let mut text = String::new();
    while r.remaining() > 0 {
        let tag = r.read_varint()?;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 0 {
            return None;
        }
        match (field, wire_type) {
            (2, 0) => progress_ms = u32::try_from(r.read_varint()?).ok()?,
            (7, 2) => {
                let len = r.read_varint()?;
                let len = usize::try_from(len).ok()?;
                let bytes = r.read_bytes(len)?;
                text = String::from_utf8_lossy(bytes).into_owned();
            }
            _ => r.skip(wire_type)?,
        }
    }
    let text = text.trim();
    if text.is_empty() {
        return Some(None);
    }
    Some(Some(DanmakuElem {
        progress_ms,
        text: text.to_string(),
    }))
}

/// 解析一个分段响应（repeated `DanmakuElem` 的裸拼接）
///
/// 容错口径：遇到无法识别的结构就停，保留已经解析出来的部分——
/// 弹幕是辅助信号，宁可少几条也不能让整轮评估失败。
pub fn parse_segment(buf: &[u8]) -> Vec<DanmakuElem> {
    let mut out = Vec::new();
    let mut r = Reader::new(buf);
    while r.remaining() > 0 {
        let Some(tag) = r.read_varint() else { break };
        let field = tag >> 3;
        let wire_type = tag & 7;
        // 外层就是 repeated field 1（length-delimited）
        if field == 1 && wire_type == 2 {
            let Some(len) = r.read_varint() else { break };
            let Some(len) = usize::try_from(len).ok() else { break };
            let Some(payload) = r.read_bytes(len) else { break };
            if let Some(Some(elem)) = parse_elem(payload) {
                out.push(elem);
            }
            continue;
        }
        if r.skip(wire_type).is_none() {
            break;
        }
    }
    out
}

/// 网络读取使用严格模式，避免把 HTTP 200 的 JSON 风控响应或截断数据当成无弹幕。
/// 容错解析 `parse_segment` 仍供离线样本使用。
pub fn parse_segment_checked(buf: &[u8]) -> Result<Vec<DanmakuElem>, String> {
    let invalid = || "弹幕响应结构异常，无法确认是否有弹幕".to_string();
    let mut out = Vec::new();
    let mut r = Reader::new(buf);
    while r.remaining() > 0 {
        let tag = r.read_varint().ok_or_else(invalid)?;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 0 {
            return Err(invalid());
        }
        if field == 1 && wire_type == 2 {
            let len = usize::try_from(r.read_varint().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            let payload = r.read_bytes(len).ok_or_else(invalid)?;
            if let Some(elem) = parse_elem(payload).ok_or_else(invalid)? {
                out.push(elem);
            }
        } else {
            r.skip(wire_type).ok_or_else(invalid)?;
        }
    }
    Ok(out)
}

/// 按分钟聚合弹幕
///
/// `window_secs` 是这次分段实际覆盖的时长（不是视频全长），
/// 摘要里要如实写，否则读的人会以为整片都看过。
pub fn digest(elems: &[DanmakuElem], window_secs: u64) -> DanmakuDigest {
    use std::collections::HashMap;

    let mut buckets: HashMap<u32, Vec<&str>> = HashMap::new();
    for elem in elems {
        let minute = elem.progress_ms / 60_000 + 1;
        buckets.entry(minute).or_default().push(elem.text.as_str());
    }

    let mut peak: Vec<MinuteBucket> = buckets
        .into_iter()
        .filter(|(_, texts)| texts.len() >= MIN_COUNT_PER_PEAK)
        .map(|(minute, texts)| {
            // 代表句只在「说得出来」的弹幕里挑；全是符号时退回原池
            let meaningful: Vec<&str> = texts
                .iter()
                .copied()
                .filter(|text| is_meaningful(text))
                .collect();
            let pool: &[&str] = if meaningful.is_empty() {
                &texts
            } else {
                &meaningful
            };
            let mut freq: HashMap<&str, usize> = HashMap::new();
            for text in pool {
                *freq.entry(*text).or_default() += 1;
            }
            // 并列时取更长的一句：字数多的通常信息量更大
            let (top_text, repeat) = freq
                .iter()
                .max_by_key(|(text, count)| (**count, text.chars().count()))
                .map(|(text, count)| (*text, *count))
                .unwrap_or(("", 0));
            let (top_text, repeat) = if repeat >= MIN_REPEAT_FOR_MEME {
                (top_text, repeat)
            } else {
                // 这一分钟没有刷屏句 → 退回最长的一条（更有描述性）
                let longest = pool
                    .iter()
                    .max_by_key(|text| text.chars().count())
                    .copied()
                    .unwrap_or("");
                (longest, 1)
            };
            MinuteBucket {
                minute,
                count: texts.len(),
                top_text: truncate_chars(top_text, MAX_TEXT_CHARS),
                repeat,
            }
        })
        .collect();

    // 密度降序（并列时早的分钟优先），取前几名后恢复时间升序
    peak.sort_by(|a, b| b.count.cmp(&a.count).then(a.minute.cmp(&b.minute)));
    peak.truncate(MAX_PEAK_MINUTES);
    peak.sort_by_key(|b| b.minute);

    DanmakuDigest {
        total: elems.len(),
        window_secs,
        peak,
    }
}

impl DanmakuDigest {
    /// 只在确实有可注入摘要时产出
    ///
    /// 空分段（一条都没取到）视为未取得——接口异常与真冷门在数据上长得一样，
    /// 都不能当成「这个视频没人看」这种可引用结论。只有一两条时仍然产出：
    /// 「前 6 分钟 2 条」本身就是有效信号（这条内容没人讨论）。
    pub fn new_if_useful(elems: &[DanmakuElem], window_secs: u64) -> Option<Self> {
        let digest = digest(elems, window_secs);
        digest.to_prompt_line()?;
        Some(digest)
    }

    /// 注入评估上下文的一行摘要
    ///
    /// 一条弹幕都没有时返回 `None` —— 由调用方按「未取得」处理，
    /// 不伪造「0 条弹幕」这种看似有据可查的结论。
    pub fn to_prompt_line(&self) -> Option<String> {
        if self.total == 0 {
            return None;
        }
        let window_min = (self.window_secs as f64 / 60.0).round().max(1.0);
        let density = (self.total as f64 / window_min).round() as usize;
        let mut line = format!(
            "前 {} 分钟 {} 条（约 {} 条/分钟）",
            window_min as u64, self.total, density
        );
        for bucket in &self.peak {
            if bucket.top_text.is_empty() {
                continue;
            }
            let times = if bucket.repeat >= MIN_REPEAT_FOR_MEME {
                format!(" ×{}", bucket.repeat)
            } else {
                String::new()
            };
            line.push_str(&format!(
                "；第 {} 分钟 {} 条「{}」{}",
                bucket.minute, bucket.count, bucket.top_text, times
            ));
        }
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_parser_distinguishes_empty_segments_from_invalid_responses() {
        assert!(parse_segment_checked(&[]).unwrap().is_empty());
        assert!(parse_segment_checked(br#"{"code":-412,"message":"blocked"}"#).is_err());
        assert!(parse_segment_checked(b"<html>blocked</html>").is_err());
        assert!(parse_segment_checked(&[0x0a, 0x03, 0x10]).is_err());
        assert!(parse_segment_checked(&[0x0a, 0x01, 0x80]).is_err());
        // 超出 u64 的 varint 与超出 u32 的时间不能回绕到合法时间。
        assert!(parse_segment_checked(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02]).is_err());
        let mut payload = vec![0x10];
        payload.extend(varint(u64::from(u32::MAX) + 1));
        let mut segment = vec![0x0a];
        segment.extend(varint(payload.len() as u64));
        segment.extend(payload);
        assert!(parse_segment_checked(&segment).is_err());
    }

    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out
    }

    /// 造一条 DanmakuElem（含 id/mode/progress/content 四个字段）
    fn elem_bytes(progress_ms: u32, text: &str) -> Vec<u8> {
        let mut inner = Vec::new();
        inner.extend(varint((1 << 3) | 0));
        inner.extend(varint(12345));
        inner.extend(varint((2 << 3) | 0));
        inner.extend(varint(progress_ms as u64));
        inner.extend(varint((3 << 3) | 0));
        inner.extend(varint(1));
        inner.extend(varint((7 << 3) | 2));
        inner.extend(varint(text.len() as u64));
        inner.extend(text.as_bytes());
        let mut out = varint((1 << 3) | 2);
        out.extend(varint(inner.len() as u64));
        out.extend(inner);
        out
    }

    fn segment(items: &[(u32, &str)]) -> Vec<u8> {
        items.iter().flat_map(|(t, s)| elem_bytes(*t, s)).collect()
    }

    #[test]
    fn parses_progress_and_text() {
        let buf = segment(&[(12_000, "第一"), (125_500, "第二")]);
        let elems = parse_segment(&buf);
        assert_eq!(elems.len(), 2);
        assert_eq!(elems[0].progress_ms, 12_000);
        assert_eq!(elems[0].text, "第一");
        assert_eq!(elems[1].progress_ms, 125_500);
    }

    #[test]
    fn skips_unknown_fields_without_losing_known_ones() {
        // 在 content 前后各插一个未知字段：float64(9, wt1) 与 string(11, wt2)
        let mut inner = Vec::new();
        inner.extend(varint((2 << 3) | 0));
        inner.extend(varint(9_000));
        inner.extend(varint((9 << 3) | 1));
        inner.extend([0u8; 8]);
        inner.extend(varint((7 << 3) | 2));
        inner.extend(varint("升级".len() as u64));
        inner.extend("升级".as_bytes());
        inner.extend(varint((11 << 3) | 2));
        inner.extend(varint(3));
        inner.extend(b"abc");
        let mut buf = varint((1 << 3) | 2);
        buf.extend(varint(inner.len() as u64));
        buf.extend(inner);

        let elems = parse_segment(&buf);
        assert_eq!(elems.len(), 1);
        assert_eq!(elems[0].progress_ms, 9_000);
        assert_eq!(elems[0].text, "升级");
    }

    #[test]
    fn truncated_payload_does_not_panic() {
        let buf = segment(&[(1_000, "完整一条"), (2_000, "被截断的这一条")]);
        // 砍掉尾部：它落在最后一条中间
        let cut = &buf[..buf.len() - 6];
        let elems = parse_segment(cut);
        assert_eq!(elems.len(), 1);
        assert_eq!(elems[0].text, "完整一条");
    }

    #[test]
    fn empty_and_blank_danmaku_are_dropped() {
        assert!(parse_segment(&[]).is_empty());
        let buf = segment(&[(0, "   ")]);
        assert!(parse_segment(&buf).is_empty());
    }

    #[test]
    fn digest_buckets_by_minute_and_prefers_repeated_line() {
        let buf = segment(&[
            (1_000, "哈哈"),
            (2_000, "前方高能"),
            (3_000, "前方高能"),
            (4_000, "前方高能"),
            (61_000, "这段我看了十遍"),
            (62_000, "这段我看了十遍"),
            (63_000, "泪目"),
        ]);
        let elems = parse_segment(&buf);
        let d = digest(&elems, 360);
        assert_eq!(d.total, 7);
        assert_eq!(d.peak.len(), 2);
        // 输出按时间升序
        assert_eq!(d.peak[0].minute, 1);
        assert_eq!(d.peak[0].count, 4);
        assert_eq!(d.peak[0].top_text, "前方高能");
        assert_eq!(d.peak[0].repeat, 3);
        assert_eq!(d.peak[1].minute, 2);
        assert_eq!(d.peak[1].repeat, 2);
    }

    #[test]
    fn digest_falls_back_to_longest_line_when_nothing_repeats() {
        let buf = segment(&[(1_000, "短"), (2_000, "这一句明显更长一些"), (3_000, "中等的")]);
        let d = digest(&parse_segment(&buf), 360);
        assert_eq!(d.peak[0].top_text, "这一句明显更长一些");
        assert_eq!(d.peak[0].repeat, 1);
    }

    #[test]
    fn sparse_minutes_never_enter_peak() {
        // 两个分钟各 1 条：都不够 MIN_COUNT_PER_PEAK
        let buf = segment(&[(1_000, "a"), (61_000, "b")]);
        let d = digest(&parse_segment(&buf), 360);
        assert_eq!(d.total, 2);
        assert!(d.peak.is_empty());
    }

    #[test]
    fn peak_is_capped_and_sorted_by_time() {
        let mut items = Vec::new();
        for minute in 1..=6u32 {
            for i in 0..minute {
                items.push(((minute - 1) * 60_000 + i * 100, "刷屏"));
            }
        }
        let refs: Vec<(u32, &str)> = items.iter().map(|(t, s)| (*t, *s)).collect();
        let d = digest(&parse_segment(&segment(&refs)), 360);
        assert_eq!(d.peak.len(), MAX_PEAK_MINUTES);
        // 取密度最高的 4/5/6 分钟，且仍按时间升序
        assert_eq!(
            d.peak.iter().map(|b| b.minute).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
    }

    #[test]
    fn no_danmaku_yields_no_prompt_line() {
        let d = digest(&[], 360);
        assert_eq!(d.total, 0);
        assert!(d.to_prompt_line().is_none());
        assert!(DanmakuDigest::new_if_useful(&[], 360).is_none());
    }

    #[test]
    fn sparse_but_present_danmaku_still_counts_as_acquired() {
        // 只有 2 条：没有高峰，但「前 6 分钟 2 条」本身就说明这条内容没人讨论
        let buf = segment(&[(1_000, "a"), (61_000, "b")]);
        let digest = DanmakuDigest::new_if_useful(&parse_segment(&buf), 360).unwrap();
        assert_eq!(digest.total, 2);
        assert!(digest.peak.is_empty());
        let line = digest.to_prompt_line().unwrap();
        assert!(line.contains("前 6 分钟 2 条"), "{}", line);
        assert!(!line.contains("第"), "无高峰时不该出现分钟条目：{}", line);
    }

    #[test]
    fn punctuation_only_line_is_not_chosen_as_representative() {
        // 「？」重复 3 次，但说不出一句话来 → 代表句改选有内容的那条，
        // 计数仍然按 4 条算（它确实是观众反应）
        let buf = segment(&[
            (1_000, "？"),
            (2_000, "？"),
            (3_000, "？"),
            (4_000, "这里讲得挺清楚"),
        ]);
        let d = digest(&parse_segment(&buf), 360);
        assert_eq!(d.peak[0].count, 4);
        assert_eq!(d.peak[0].top_text, "这里讲得挺清楚");
        assert_eq!(d.peak[0].repeat, 1);
    }

    #[test]
    fn all_punctuation_falls_back_to_raw_pool() {
        let buf = segment(&[(1_000, "？"), (2_000, "？？"), (3_000, "？？？")]);
        let d = digest(&parse_segment(&buf), 360);
        assert_eq!(d.peak[0].top_text, "？？？");
    }

    #[test]
    fn parses_live_segment_sample_when_provided() {
        // 可选的本地真实样本（harness 检测到 tmp/_seg.bin 时才注入）：
        // 用来确认线上 seg.so 的字节结构仍然对得上，不替代上面的构造用例
        let Ok(path) = std::env::var("DANMAKU_SEGMENT_FILE") else {
            return;
        };
        let buf = std::fs::read(&path).expect("样本文件读不到");
        let elems = parse_segment(&buf);
        assert!(!elems.is_empty(), "真实样本解析出 0 条");
        assert_eq!(parse_segment_checked(&buf).expect("真实样本应通过严格解析"), elems);
        // 第一段只覆盖前 6 分钟
        assert!(
            elems.iter().all(|e| e.progress_ms < 360_000),
            "第一段出现超出 6 分钟的时间戳"
        );
        // 弹幕正文必须是可读文本，不是乱码字节
        assert!(elems.iter().any(|e| e.text.chars().count() > 1));
        let digest = digest(&elems, 360);
        assert!(!digest.to_prompt_line().unwrap().is_empty());
    }

    #[test]
    fn prompt_line_states_window_and_peaks() {
        let buf = segment(&[
            (1_000, "前方高能"),
            (2_000, "前方高能"),
            (3_000, "前方高能"),
            (61_000, "泪目"),
            (62_000, "泪目"),
            (63_000, "泪目"),
        ]);
        let line = digest(&parse_segment(&buf), 360).to_prompt_line().unwrap();
        assert!(line.contains("前 6 分钟 6 条"), "{}", line);
        assert!(line.contains("第 1 分钟 3 条「前方高能」 ×3"), "{}", line);
    }
}
