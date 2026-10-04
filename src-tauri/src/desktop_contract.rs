//! Platform-independent desktop operation settings and coordinate contract.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub user_idle_ms: u64,
    pub max_yield_wait_ms: u64,
    pub operation_timeout_ms: u64,
}
impl Default for DesktopConfig {
    fn default() -> Self {
        Self { user_idle_ms: 750, max_yield_wait_ms: 15_000, operation_timeout_ms: 15_000 }
    }
}
impl DesktopConfig {
    pub fn bounded(&self) -> Self {
        Self {
            user_idle_ms: self.user_idle_ms.clamp(250, 5_000),
            max_yield_wait_ms: self.max_yield_wait_ms.clamp(1_000, 30_000),
            operation_timeout_ms: self.operation_timeout_ms.clamp(1_000, 30_000),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScreenBounds {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}
impl ScreenBounds {
    pub fn contains(&self, x: i64, y: i64) -> bool {
        self.width > 0 && self.height > 0
            && x >= self.left as i64 && y >= self.top as i64
            && x < self.left as i64 + self.width as i64
            && y < self.top as i64 + self.height as i64
    }
}

pub fn settings_schema() -> serde_json::Value {
    let defaults = DesktopConfig::default();
    serde_json::json!([
        {"key":"tools.desktop.user_idle_ms","type":"integer","min":250,"max":5000,"step":50,"default":defaults.user_idle_ms,
         "title":{"zh":"操作前空闲时间（毫秒）","en":"Idle time before input (ms)","ja":"入力前のアイドル時間（ミリ秒）"}},
        {"key":"tools.desktop.max_yield_wait_ms","type":"integer","min":1000,"max":30000,"step":500,"default":defaults.max_yield_wait_ms,
         "title":{"zh":"等待用户操作结束（毫秒）","en":"Maximum wait for user input (ms)","ja":"ユーザー操作の最大待機時間（ミリ秒）"}},
        {"key":"tools.desktop.operation_timeout_ms","type":"integer","min":1000,"max":30000,"step":500,"default":defaults.operation_timeout_ms,
         "title":{"zh":"电脑操作超时（毫秒）","en":"Desktop operation timeout (ms)","ja":"デスクトップ操作のタイムアウト（ミリ秒）"}}
    ])
}

#[derive(Debug, Serialize)]
pub struct ObservationReceipt {
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OperationReceipt {
    pub operation_completed: bool,
    pub goal_verified: bool,
    pub action_result: Option<serde_json::Value>,
    pub observation: Option<ObservationReceipt>,
}

/// Input executes once. Observation failures preserve its outcome and never replay input.
pub async fn run_observed_operation<I,O>(input:I,observation:O,observe_after:bool) -> Result<OperationReceipt,String>
where I: std::future::Future<Output=Result<Option<serde_json::Value>,String>>,
      O: std::future::Future<Output=Result<Option<serde_json::Value>,String>> {
    let action_result=input.await?;
    let observation=if observe_after {
        Some(match observation.await {
            Ok(data)=>ObservationReceipt {success:true,data,error:None},
            Err(error)=>ObservationReceipt {success:false,data:None,error:Some(error)},
        })
    } else {None};
    Ok(OperationReceipt {operation_completed:true,goal_verified:false,action_result,observation})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn observation_failure_preserves_input_and_never_replays_it() {
        let events=std::sync::Mutex::new(Vec::new());
        let receipt=run_observed_operation(
            async {events.lock().unwrap().push("input");Ok(Some(serde_json::json!({"sent":true})))},
            async {events.lock().unwrap().push("observe");Err("vision unavailable".into())},true).await.unwrap();
        assert_eq!(*events.lock().unwrap(),vec!["input","observe"]);
        assert!(receipt.operation_completed);
        assert!(!receipt.goal_verified);
        assert!(!receipt.observation.unwrap().success);
    }
    #[tokio::test]
    async fn failed_input_stops_before_observation_and_disabled_observation_is_not_run() {
        let failed=run_observed_operation(async {Err("input timeout".into())},async {panic!("must not observe failed input")},true).await;
        assert!(failed.is_err());
        let receipt=run_observed_operation(async {Ok(None)},async {panic!("observation disabled")},false).await.unwrap();
        assert!(receipt.observation.is_none());
        assert!(!receipt.goal_verified);
    }
    #[test]
    fn negative_monitor_coordinates_and_edges() {
        let bounds = ScreenBounds {left:-1920,top:-200,width:3840,height:1280};
        assert!(bounds.contains(-1920,-200));
        assert!(bounds.contains(1919,1079));
        assert!(!bounds.contains(1920,0));
        assert!(!bounds.contains(-1921,0));
        assert!(!bounds.contains(i64::MAX,0));
        assert!(!ScreenBounds { width:0,..bounds }.contains(0,0));
    }
    #[test]
    fn legacy_defaults_and_bounded_operator_settings() {
        let config: DesktopConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.user_idle_ms,DesktopConfig::default().user_idle_ms);
        let bounded=DesktopConfig {user_idle_ms:0,max_yield_wait_ms:u64::MAX,operation_timeout_ms:0}.bounded();
        assert_eq!((bounded.user_idle_ms,bounded.max_yield_wait_ms,bounded.operation_timeout_ms),(250,30000,1000));
    }
}
