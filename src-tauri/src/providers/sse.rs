//! Shared SSE framing and stream transport; adapters only interpret JSON.
//! Inspired by cc-switch's transport/adapter separation, independently implemented.
use std::collections::VecDeque;
use std::time::Duration;

use futures::{stream::BoxStream, Stream, StreamExt};
use serde_json::Value;

const MAX_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub data: String,
    pub json: Option<Value>,
    pub event: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct StreamTimeouts {
    pub first_byte: Duration,
    pub idle: Duration,
}

impl Default for StreamTimeouts {
    fn default() -> Self {
        Self {
            first_byte: Duration::from_secs(60),
            idle: Duration::from_secs(120),
        }
    }
}

/// Byte framing preserves UTF-8 even when network chunks split a code point.
/// An empty prefix opts into newline-delimited JSON.
pub struct SseDecoder {
    prefix: String,
    line: Vec<u8>,
    data: Vec<String>,
    event: Option<String>,
    event_bytes: usize,
    skip_lf: bool,
    first_line: bool,
}

impl SseDecoder {
    pub fn new(prefix: Option<&str>) -> Self {
        Self {
            prefix: prefix.unwrap_or("data:").trim_end().to_string(),
            line: Vec::new(),
            data: Vec::new(),
            event: None,
            event_bytes: 0,
            skip_lf: false,
            first_line: true,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, String> {
        let mut events = Vec::new();
        for &byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.read_line(&mut events)?;
                self.skip_lf = byte == b'\r';
            } else {
                self.line.push(byte);
                if self.line.len() + self.event_bytes > MAX_EVENT_BYTES {
                    return Err("SSE event exceeds the 1 MiB size limit".into());
                }
            }
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<SseEvent>, String> {
        let mut events = Vec::new();
        if !self.line.is_empty() {
            self.read_line(&mut events)?;
        }
        self.dispatch(&mut events)?;
        Ok(events)
    }

    fn read_line(&mut self, events: &mut Vec<SseEvent>) -> Result<(), String> {
        let bytes = std::mem::take(&mut self.line);
        let line =
            std::str::from_utf8(&bytes).map_err(|_| "Invalid UTF-8 in SSE response".to_string())?;
        let line = if self.first_line {
            self.first_line = false;
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if self.prefix.is_empty() {
            if !line.trim().is_empty() {
                self.data.push(line.trim().to_owned());
                self.dispatch(events)?;
            }
        } else if line.is_empty() {
            self.dispatch(events)?;
        } else if let Some(data) = line.strip_prefix(&self.prefix) {
            let data = data.strip_prefix(' ').unwrap_or(data);
            self.event_bytes += data.len() + 1;
            if self.event_bytes > MAX_EVENT_BYTES {
                return Err("SSE event exceeds the 1 MiB size limit".into());
            }
            self.data.push(data.to_owned());
        } else if let Some(name) = line.strip_prefix("event:") {
            self.event = Some(name.trim().to_owned());
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) -> Result<(), String> {
        let event = self.event.take();
        self.event_bytes = 0;
        if self.data.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.data).join("\n");
        let json = serde_json::from_str::<Value>(&data).ok();
        if json.is_none() && data.trim_start().starts_with('{') {
            return Err("Malformed JSON in SSE response".into());
        }
        events.push(SseEvent { data, json, event });
        Ok(())
    }
}

fn stream_error(event: &SseEvent) -> Option<String> {
    let value = event.json.as_ref();
    let kind = value
        .and_then(|v| v["type"].as_str())
        .or(event.event.as_deref());
    let error = value.and_then(|v| {
        v.get("error").filter(|error| !error.is_null()).or_else(|| {
            v.pointer("/response/error")
                .filter(|error| !error.is_null())
        })
    });
    if error.is_none() && !matches!(kind, Some("error" | "response.failed")) {
        return None;
    }
    let message = error
        .or(value)
        .map(Value::to_string)
        .unwrap_or_else(|| "Upstream stream failed".into());
    Some(message.chars().take(400).collect())
}

/// Dropping the consumer drops the HTTP body; no background reader or unbounded queue.
pub fn event_stream<S, B, E>(
    stream: S,
    prefix: Option<String>,
    timeouts: StreamTimeouts,
) -> BoxStream<'static, Result<SseEvent, String>>
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let state = (
        Box::pin(stream),
        SseDecoder::new(prefix.as_deref()),
        VecDeque::new(),
        false,
        false,
    );
    futures::stream::unfold(
        state,
        move |(mut stream, mut decoder, mut queued, mut started, mut ended)| async move {
            loop {
                if let Some(event) = queued.pop_front() {
                    if let Some(error) = stream_error(&event) {
                        return Some((
                            Err(error),
                            (stream, decoder, VecDeque::new(), started, true),
                        ));
                    }
                    return Some((Ok(event), (stream, decoder, queued, started, ended)));
                }
                if ended {
                    return None;
                }
                let timeout = if started {
                    timeouts.idle
                } else {
                    timeouts.first_byte
                };
                let events = match tokio::time::timeout(timeout, stream.next()).await {
                    Err(_) => Err(if started {
                        "SSE stream idle timeout"
                    } else {
                        "SSE first-byte timeout"
                    }
                    .into()),
                    Ok(Some(Err(error))) => Err(error.to_string()),
                    Ok(Some(Ok(chunk))) => {
                        started = true;
                        decoder.push(chunk.as_ref())
                    }
                    Ok(None) => {
                        ended = true;
                        decoder.finish()
                    }
                };
                match events {
                    Ok(events) => queued.extend(events),
                    Err(error) => {
                        return Some((
                            Err(error),
                            (stream, decoder, VecDeque::new(), started, true),
                        ))
                    }
                }
            }
        },
    )
    .boxed()
}

/// Transitional bridge for native JSON interpreters: each chunk is a complete,
/// UTF-8-safe frame with canonical spacing and LF separators.
pub fn normalize_sse<S, B, E>(stream: S) -> BoxStream<'static, Result<Vec<u8>, String>>
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    event_stream(stream, None, StreamTimeouts::default())
        .map(|event| {
            event.map(|event| {
                let data = event
                    .json
                    .map(|json| json.to_string())
                    .unwrap_or(event.data);
                format!("data: {data}\n\n").into_bytes()
            })
        })
        .boxed()
}

pub async fn read_sse<S, B, E, F>(
    stream: S,
    event_path: Option<&str>,
    done_sentinel: Option<&str>,
    mut on_event: impl FnMut(SseEvent) -> F + Send,
) -> Result<(), String>
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
    F: std::future::Future<Output = bool> + Send,
{
    let mut stream = event_stream(
        stream,
        event_path.map(str::to_owned),
        StreamTimeouts::default(),
    );
    while let Some(event) = stream.next().await {
        let event = event?;
        if done_sentinel.is_some_and(|sentinel| event.data.trim() == sentinel) {
            return Ok(());
        }
        if !on_event(event).await {
            return Ok(());
        }
    }
    Ok(())
}

/// Native adapter readers must stop even when upstream is idle and the UI cancels.
pub fn normalize_sse_until_closed<S, B, E>(
    stream: S,
    sender: tokio::sync::mpsc::Sender<super::base::StreamEvent>,
) -> BoxStream<'static, Result<Vec<u8>, String>>
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    normalize_sse(stream)
        .take_until(async move { sender.closed().await })
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_byte_boundary_preserves_unicode_crlf_and_multiline_json() {
        let input = "\u{feff}: keepalive\r\nevent: delta\r\ndata:{\"text\":\"你好😀\",\r\ndata: \"n\":1}\r\n\r\n";
        for split in 0..=input.len() {
            let mut decoder = SseDecoder::new(None);
            let mut events = decoder.push(&input.as_bytes()[..split]).unwrap();
            events.extend(decoder.push(&input.as_bytes()[split..]).unwrap());
            events.extend(decoder.finish().unwrap());
            assert_eq!(events.len(), 1, "split={split}");
            assert_eq!(events[0].json, Some(json!({"text":"你好😀", "n":1})));
            assert_eq!(events[0].event.as_deref(), Some("delta"));
        }
    }

    #[test]
    fn eof_flushes_and_invalid_or_oversized_frames_fail() {
        let mut decoder = SseDecoder::new(None);
        assert!(decoder.push(b"data: {\"a\":1}").unwrap().is_empty());
        assert_eq!(decoder.finish().unwrap()[0].json, Some(json!({"a":1})));
        assert!(SseDecoder::new(None).push(b"data: {bad}\n\n").is_err());
        assert!(SseDecoder::new(None)
            .push(&vec![b'x'; MAX_EVENT_BYTES + 1])
            .is_err());
        assert!(SseDecoder::new(None).push(b"data: \xff\n\n").is_err());
    }

    #[tokio::test]
    async fn async_delivery_survives_bounded_channel_backpressure() {
        let input = futures::stream::iter(
            (0..128).map(|n| Ok::<_, String>(format!("data: {{\"n\":{n}}}\n\n").into_bytes())),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let producer = tokio::spawn(async move {
            read_sse(input, None, None, |event| {
                let tx = tx.clone();
                async move { tx.send(event).await.is_ok() }
            })
            .await
        });
        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 128);
        producer.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn upstream_errors_and_first_byte_timeout_are_explicit() {
        let input = futures::stream::iter([Ok::<_, String>(
            b"event: error\ndata: {\"error\":{\"message\":\"overloaded\"}}\n\n".to_vec(),
        )]);
        assert!(event_stream(input, None, StreamTimeouts::default())
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .contains("overloaded"));
        let mut stream = event_stream(
            futures::stream::pending::<Result<Vec<u8>, String>>(),
            None,
            StreamTimeouts {
                first_byte: Duration::from_millis(5),
                idle: Duration::from_millis(5),
            },
        );
        assert!(stream
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .contains("first-byte"));
        assert!(stream.next().await.is_none());
    }
}
