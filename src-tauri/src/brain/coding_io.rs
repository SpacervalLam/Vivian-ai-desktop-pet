//! Ordered disk work outside the async agent and session locks. Flush is an
//! explicit durability barrier for backup, readers and application shutdown.
use std::sync::{mpsc, LazyLock};

type Job = Box<dyn FnOnce() -> Result<(), String> + Send>;
enum Message {
    Write(Job),
    Flush(mpsc::SyncSender<Result<(), String>>),
}
struct Writer {
    tx: mpsc::SyncSender<Message>,
}
impl Writer {
    fn new() -> Self {
        // Preserve records with bounded backpressure instead of growing an
        // unbounded queue of screenshots when storage falls behind.
        let (tx, rx) = mpsc::sync_channel::<Message>(64);
        std::thread::Builder::new()
            .name("work-persistence".into())
            .spawn(move || {
                let mut failures = Vec::new();
                for message in rx {
                    match message {
                        Message::Write(job) => {
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                                    .unwrap_or_else(
                                        |_| Err("Work persistence job panicked".into()),
                                    );
                            if let Err(error) = result {
                                tracing::error!("Work persistence failed: {error}");
                                if failures.len() < 16 {
                                    failures.push(error);
                                }
                            }
                        }
                        Message::Flush(reply) => {
                            let result = if failures.is_empty() {
                                Ok(())
                            } else {
                                Err(failures.join("; "))
                            };
                            failures.clear();
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .expect("start work persistence writer");
        Self { tx }
    }
    fn enqueue(&self, job: Job) -> Result<(), String> {
        self.tx
            .send(Message::Write(job))
            .map_err(|_| "Work persistence writer stopped".into())
    }
    fn flush(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Message::Flush(tx))
            .map_err(|_| "Work persistence writer stopped")?;
        rx.recv()
            .map_err(|_| "Work persistence flush interrupted")?
    }
}
static WRITER: LazyLock<Writer> = LazyLock::new(Writer::new);

pub fn enqueue(job: impl FnOnce() -> Result<(), String> + Send + 'static) -> Result<(), String> {
    WRITER.enqueue(Box::new(job))
}
pub fn flush() -> Result<(), String> {
    WRITER.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flush_waits_for_ordered_writes_and_reports_failure() {
        let state = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = Writer::new();
        for i in 0..3 {
            let state = state.clone();
            writer
                .enqueue(Box::new(move || {
                    state.lock().unwrap().push(i);
                    Ok(())
                }))
                .unwrap();
        }
        writer.flush().unwrap();
        assert_eq!(*state.lock().unwrap(), [0, 1, 2]);
        writer
            .enqueue(Box::new(|| Err("disk unavailable".into())))
            .unwrap();
        assert!(writer.flush().unwrap_err().contains("disk unavailable"));
        writer.flush().unwrap();
    }
}
