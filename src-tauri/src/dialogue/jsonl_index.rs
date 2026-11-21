//! Rebuildable offsets: history remains the source of truth, pages read only selected bodies.
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Serialize, Deserialize)]
struct Span {
    start: u64,
    len: u64,
    id: String,
    timestamp: f64,
    utterance: Option<String>,
    role: String,
    channel: Option<String>,
    visible: bool,
}
#[derive(Default, Serialize, Deserialize)]
pub struct HistoryIndex {
    length: u64,
    modified: u128,
    spans: Vec<Span>,
}
pub fn file_modified(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}
fn accepts(role: &str, channel: Option<&str>, visible: bool, scope: Option<&str>) -> bool {
    match scope {
        None => true,
        Some("wechat") => {
            visible && role != "system" && (channel.is_none() || channel == Some("wechat"))
        }
        Some(scope) => {
            visible
                && role != "system"
                && channel == Some(scope.strip_suffix("_exact").unwrap_or(scope))
        }
    }
}
impl HistoryIndex {
    pub fn restore(path: &Path) -> Self {
        let sidecar = path.with_extension("offsets.json");
        let mut index: Self = std::fs::read(&sidecar)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if let Err(error) = index.refresh(path) {
            tracing::warn!("[HistoryIndex] rebuild failed: {error}");
        }
        index
    }
    pub fn refresh(&mut self, path: &Path) -> std::io::Result<()> {
        self.refresh_inner(path, None)
    }
    pub fn appended(
        &mut self,
        path: &Path,
        original_length: u64,
        original_modified: u128,
    ) -> std::io::Result<()> {
        self.refresh_inner(path, Some((original_length, original_modified)))
    }
    fn refresh_inner(
        &mut self,
        path: &Path,
        known_append: Option<(u64, u128)>,
    ) -> std::io::Result<()> {
        let length = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                *self = Self::default();
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let stamp = file_modified(path);
        if length == self.length && stamp == self.modified {
            return Ok(());
        }
        // Only an app append with the exact prior file version may reuse offsets.
        // External rewrites, including growing rewrites, always rebuild them.
        if length <= self.length || known_append != Some((self.length, self.modified)) {
            *self = Self::default();
        }
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.length))?;
        let mut reader = BufReader::new(file);
        let mut offset = self.length;
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            let read = reader.read_until(b'\n', &mut bytes)?;
            if read == 0 {
                break;
            }
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let (Some(id), Some(timestamp)) =
                    (value["id"].as_str(), value["timestamp"].as_f64())
                {
                    if timestamp.is_finite() {
                        self.spans.push(Span {
                            start: offset,
                            len: read as u64,
                            id: id.into(),
                            timestamp,
                            utterance: value["metadata"]["utterance_id"]
                                .as_str()
                                .map(str::to_owned),
                            role: value["role"].as_str().unwrap_or("").into(),
                            channel: value["metadata"]["channel"].as_str().map(str::to_owned),
                            visible: value["metadata"]["source"].as_str() != Some("quiet_control"),
                        });
                    }
                }
            }
            offset += read as u64;
        }
        self.length = length;
        self.modified = stamp;
        // Keep timestamp ordering, including older turns appended by delayed deliveries.
        self.spans.sort_by(|a, b| {
            a.timestamp
                .total_cmp(&b.timestamp)
                .then_with(|| a.start.cmp(&b.start))
        });
        if let Ok(text) = serde_json::to_string(self) {
            if let Err(error) =
                crate::utils::fs::write_atomic(&path.with_extension("offsets.json"), &text)
            {
                tracing::warn!("[HistoryIndex] sidecar persistence failed: {error}");
            }
        }
        Ok(())
    }
    pub fn contains(&self, id: &str) -> bool {
        self.spans.iter().any(|span| span.id == id)
    }
    pub fn utterances(&self) -> impl Iterator<Item = &str> {
        self.spans.iter().filter_map(|s| s.utterance.as_deref())
    }
    pub fn count<T: Serialize>(&self, pending: &[T]) -> usize {
        self.spans.len() + self.pending(pending).len()
    }
    fn pending<T: Serialize>(&self, pending: &[T]) -> Vec<(usize, f64)> {
        let ids: std::collections::HashSet<_> = self.spans.iter().map(|s| s.id.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        pending
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let value = serde_json::to_value(item).ok()?;
                let id = value["id"].as_str()?;
                if ids.contains(id) || !seen.insert(id.to_string()) {
                    return None;
                }
                Some((i, value["timestamp"].as_f64()?))
            })
            .collect()
    }
    pub fn unread<T: Serialize>(&self, pending: &[T], channel: &str, watermark: f64) -> usize {
        let stored = self
            .spans
            .iter()
            .filter(|s| {
                s.role == "assistant"
                    && s.visible
                    && s.channel.as_deref() == Some(channel)
                    && s.timestamp > watermark
            })
            .count();
        stored
            + self
                .pending(pending)
                .iter()
                .filter(|(i, t)| {
                    let value = serde_json::to_value(&pending[*i]).unwrap_or_default();
                    *t > watermark
                        && value["role"] == "assistant"
                        && value["metadata"]["channel"].as_str() == Some(channel)
                        && value["metadata"]["source"].as_str() != Some("quiet_control")
                })
                .count()
    }
    pub fn page<T: Serialize + DeserializeOwned + Clone>(
        &self,
        path: &Path,
        pending: &[T],
        offset: usize,
        limit: usize,
        recent: bool,
    ) -> std::io::Result<(Vec<T>, bool, u64)> {
        self.page_scoped(path, pending, offset, limit, recent, None, None)
    }
    pub fn page_scoped<T: Serialize + DeserializeOwned + Clone>(
        &self,
        path: &Path,
        pending: &[T],
        offset: usize,
        limit: usize,
        recent: bool,
        before: Option<&str>,
        scope: Option<&str>,
    ) -> std::io::Result<(Vec<T>, bool, u64)> {
        let mut order: Vec<_> = self
            .spans
            .iter()
            .enumerate()
            .map(|(i, s)| (s.timestamp, false, i))
            .collect();
        order.extend(self.pending(pending).into_iter().map(|(i, t)| (t, true, i)));
        order.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        order.retain(|(_, buffered, i)| {
            if *buffered {
                let value = serde_json::to_value(&pending[*i]).unwrap_or_default();
                accepts(
                    value["role"].as_str().unwrap_or(""),
                    value["metadata"]["channel"].as_str(),
                    value["metadata"]["source"].as_str() != Some("quiet_control"),
                    scope,
                )
            } else {
                let span = &self.spans[*i];
                accepts(&span.role, span.channel.as_deref(), span.visible, scope)
            }
        });
        let total = if let Some(id) = before {
            match order.iter().position(|(_, buffered, i)| {
                if *buffered {
                    serde_json::to_value(&pending[*i])
                        .ok()
                        .is_some_and(|v| v["id"].as_str() == Some(id))
                } else {
                    self.spans[*i].id == id
                }
            }) {
                Some(position) => position,
                None => return Ok((Vec::new(), false, 0)),
            }
        } else {
            order.len()
        };
        let start = if recent {
            total.saturating_sub(limit)
        } else {
            offset.min(total)
        };
        let end = start.saturating_add(limit).min(total);
        let mut file = if self.spans.is_empty() {
            None
        } else {
            Some(File::open(path)?)
        };
        let mut result = Vec::new();
        let mut read_bytes = 0;
        for (_, buffered, index) in &order[start..end] {
            if *buffered {
                result.push(pending[*index].clone());
                continue;
            }
            let span = &self.spans[*index];
            let file = file.as_mut().unwrap();
            file.seek(SeekFrom::Start(span.start))?;
            let mut bytes = vec![0; span.len as usize];
            file.read_exact(&mut bytes)?;
            read_bytes += span.len;
            let item = serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            result.push(item);
        }
        Ok((
            result,
            if recent { start > 0 } else { end < total },
            read_bytes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_pages_read_only_selected_bodies_and_merge_pending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let text=(0..1000).map(|i| serde_json::json!({"id":i.to_string(),"timestamp":i as f64,"content":"文".repeat(4096)}).to_string()+"\n").collect::<String>();
        std::fs::write(&path, &text).unwrap();
        let index = HistoryIndex::restore(&path);
        let pending = vec![
            serde_json::json!({"id":"999","timestamp":999.0}),
            serde_json::json!({"id":"pending","timestamp":1000.0}),
        ];
        let (page, more, bytes) = index
            .page::<serde_json::Value>(&path, &pending, 0, 30, true)
            .unwrap();
        assert_eq!(page.len(), 30);
        assert!(more);
        assert_eq!(page.last().unwrap()["id"], "pending");
        assert!(bytes < text.len() as u64 / 20);
        let (first, more, _) = index
            .page::<serde_json::Value>(&path, &[], 0, 30, false)
            .unwrap();
        assert!(more);
        assert_eq!(first[0]["id"], "0");
        eprintln!(
            "history baseline={} body bytes, recent page={} body bytes",
            text.len(),
            bytes
        );
    }
    #[test]
    fn cursors_filter_metadata_before_reading_and_unread_needs_no_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let records: Vec<_>=(0..100).map(|i|serde_json::json!({"id":i.to_string(),"timestamp":i as f64,"role":"assistant","content":"正文".repeat(100),"metadata":{"channel":if i%2==0{"wechat"}else{"cross_character"}}})).collect();
        std::fs::write(
            &path,
            records
                .iter()
                .map(|v| v.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let index = HistoryIndex::restore(&path);
        let (recent, more, bytes) = index
            .page_scoped::<serde_json::Value>(&path, &[], 0, 20, true, None, Some("wechat"))
            .unwrap();
        assert!(more);
        assert_eq!(recent.len(), 20);
        assert_eq!(recent[0]["id"], "60");
        assert!(bytes > 0);
        let (older, more, _) = index
            .page_scoped::<serde_json::Value>(&path, &[], 0, 20, true, Some("60"), Some("wechat"))
            .unwrap();
        assert!(more);
        assert_eq!(older[0]["id"], "20");
        assert_eq!(older.last().unwrap()["id"], "58");
        assert_eq!(index.unread::<serde_json::Value>(&[], "wechat", 90.0), 4);
        assert!(index
            .page_scoped::<serde_json::Value>(
                &path,
                &[],
                0,
                20,
                true,
                Some("missing"),
                Some("wechat")
            )
            .unwrap()
            .0
            .is_empty());
    }

    #[test]
    fn growing_rewrite_invalidates_old_offsets_and_corrupt_sidecar_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(&path, "{\"id\":\"a\",\"timestamp\":1}\n").unwrap();
        let mut index = HistoryIndex::restore(&path);
        std::fs::write(
            &path,
            "{\"id\":\"replacement-long-id\",\"timestamp\":2}\n{\"id\":\"new\",\"timestamp\":3}\n",
        )
        .unwrap();
        index.refresh(&path).unwrap();
        assert!(!index.contains("a"));
        assert!(index.contains("replacement-long-id"));
        std::fs::write(path.with_extension("offsets.json"), "broken").unwrap();
        let restored = HistoryIndex::restore(&path);
        assert!(restored.contains("new"));
        assert_eq!(restored.count::<serde_json::Value>(&[]), 2);
    }
    #[test]
    fn offsets_rebuild_after_rewrite_and_skip_damaged_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(&path, "bad\n{\"id\":\"a\",\"timestamp\":2}\n").unwrap();
        let mut index = HistoryIndex::restore(&path);
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap(),
            "{{\"id\":\"b\",\"timestamp\":1}}"
        )
        .unwrap();
        index.refresh(&path).unwrap();
        let (page, _, _) = index
            .page::<serde_json::Value>(&path, &[], 0, 10, false)
            .unwrap();
        assert_eq!(page[0]["id"], "b");
        std::fs::write(&path, "{\"id\":\"c\",\"timestamp\":3}\n").unwrap();
        index.refresh(&path).unwrap();
        assert_eq!(index.count::<serde_json::Value>(&[]), 1);
        assert_eq!(
            index
                .page::<serde_json::Value>(&path, &[], 0, 10, false)
                .unwrap()
                .0[0]["id"],
            "c"
        );
    }
}
