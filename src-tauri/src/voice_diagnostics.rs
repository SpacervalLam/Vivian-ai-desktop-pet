//! Bounded, transcript-free stage timings. Samples never combine different stages.
use std::{collections::{HashMap, VecDeque}, sync::LazyLock, time::Instant};
use parking_lot::Mutex;
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct Sample { pub mode: String, pub stage: String, pub elapsed_ms: f64 }
#[derive(Default)]
pub struct Ledger { pending: HashMap<(String, String, String), u64>, samples: VecDeque<Sample> }
impl Ledger {
    pub fn begin(&mut self, mode: &str, key: &str, stage: &str, now: u64) {
        self.pending.retain(|_, start| now.saturating_sub(*start) < 300_000);
        if self.pending.len() >= 128 { self.pending.clear(); }
        self.pending.insert((mode.into(), key.into(), stage.into()), now);
    }
    pub fn finish(&mut self, mode: &str, key: &str, stage: &str, now: u64) {
        let Some(start) = self.pending.remove(&(mode.into(), key.into(), stage.into())) else { return; };
        if now < start || now - start > 300_000 { return; }
        self.samples.push_back(Sample { mode:mode.into(), stage:stage.into(), elapsed_ms:(now-start) as f64 });
        while self.samples.len() > 200 { self.samples.pop_front(); }
    }
    pub fn report(&self) -> serde_json::Value {
        let mut groups: HashMap<String, Vec<f64>> = HashMap::new();
        for sample in &self.samples { groups.entry(format!("{}:{}",sample.mode,sample.stage)).or_default().push(sample.elapsed_ms); }
        let groups: std::collections::BTreeMap<_,_> = groups.into_iter().map(|(key, mut values)| {
            values.sort_by(f64::total_cmp);
            let percentile = |p:f64| values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1)];
            (key, serde_json::json!({"count":values.len(),"p50_ms":percentile(0.5),"p95_ms":percentile(0.95)}))
        }).collect();
        serde_json::json!({"samples":self.samples,"groups":groups})
    }
}
static CLOCK: LazyLock<Instant> = LazyLock::new(Instant::now);
static LEDGER: LazyLock<Mutex<Ledger>> = LazyLock::new(Default::default);
pub fn now_ms() -> u64 { CLOCK.elapsed().as_millis() as u64 + 1 }
pub fn begin(mode:&str, key:&str, stage:&str) { LEDGER.lock().begin(mode,key,stage,now_ms()); }
pub fn record(mode:&str, stage:&str, start:u64, end:u64) {
    if start == 0 || end == 0 { return; }
    let mut ledger = LEDGER.lock();
    ledger.begin(mode,"playback",stage,start); ledger.finish(mode,"playback",stage,end);
}
pub fn finish(mode:&str, key:&str, stage:&str) { LEDGER.lock().finish(mode,key,stage,now_ms()); }
pub fn report() -> serde_json::Value { LEDGER.lock().report() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolation_dedup_timeout_and_stage_percentiles() {
        let mut ledger=Ledger::default();
        ledger.begin("chat","a","first_chunk",10); ledger.begin("chat","b","first_chunk",30);
        ledger.finish("chat","b","first_chunk",80); ledger.finish("chat","a","first_chunk",110);
        ledger.finish("chat","a","first_chunk",999);
        ledger.begin("tts","a","playback",100); ledger.finish("tts","a","playback",120);
        let report=ledger.report();
        assert_eq!(report["samples"].as_array().unwrap().len(),3);
        assert_eq!(report["groups"]["chat:first_chunk"]["p95_ms"],100.0);
        assert_eq!(report["groups"]["tts:playback"]["p50_ms"],20.0);
        ledger.begin("chat","stale","first_chunk",1); ledger.finish("chat","stale","first_chunk",400_000);
        assert_eq!(ledger.samples.len(),3);
        for index in 0..250 { ledger.begin("tts","bounded","playback",index); ledger.finish("tts","bounded","playback",index+1); }
        assert_eq!(ledger.samples.len(),200);
    }
}
