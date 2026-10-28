//! Live integration only: creates and destroys its own native test window.
use super::*;
use ::windows::{
    core::w,
    Win32::{
        Foundation::{HMODULE, HWND},
        UI::WindowsAndMessaging::*,
    },
};

struct Fixture {
    window: WindowTarget,
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(
                self.thread_id,
                WM_QUIT,
                Default::default(),
                Default::default(),
            );
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn fixture() -> Fixture {
    let (tx, rx) = std::sync::mpsc::channel();
    let join = std::thread::spawn(move || unsafe {
        let h = CreateWindowExW(
            Default::default(),
            w!("STATIC"),
            w!("Vivian native integration fixture"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            480,
            260,
            None,
            None,
            Some(HMODULE::default().into()),
            None,
        )
        .unwrap();
        let _edit = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            w!("initial"),
            WS_CHILD | WS_VISIBLE | WS_BORDER,
            20,
            40,
            320,
            32,
            Some(h),
            Some(HMENU(1001usize as *mut _)),
            None,
            None,
        )
        .unwrap();
        let _password = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            w!("fixture-secret"),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_PASSWORD as u32),
            20,
            90,
            320,
            32,
            Some(h),
            Some(HMENU(1002usize as *mut _)),
            None,
            None,
        )
        .unwrap();
        let _button = CreateWindowExW(
            Default::default(),
            w!("BUTTON"),
            w!("Fixture button"),
            WS_CHILD | WS_VISIBLE,
            20,
            140,
            160,
            32,
            Some(h),
            Some(HMENU(1003usize as *mut _)),
            None,
            None,
        )
        .unwrap();
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        tx.send((
            WindowTarget {
                id: h.0 as usize as i64,
                pid,
            },
            ::windows::Win32::System::Threading::GetCurrentThreadId(),
        ))
        .unwrap();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = DestroyWindow(h);
    });
    let (window, thread_id) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    Fixture {
        window,
        thread_id,
        join: Some(join),
    }
}
async fn send(owner: &str, request: Request) -> Result<Reply, String> {
    super::request(
        owner.into(),
        request,
        DesktopConfig {
            user_idle_ms: 250,
            max_yield_wait_ms: 3000,
            operation_timeout_ms: 15000,
        },
    )
    .await
}
async fn observe(w: WindowTarget) -> Reply {
    send(
        "native-smoke",
        Request::Observe {
            window: w,
            include_text: true,
            include_screenshot: true,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "live Windows desktop: creates a temporary fixture and sends input only to it"]
async fn native_window_smoke() {
    let f = fixture();
    let w = f.window;
    let state = observe(w).await;
    assert!(
        state.data["accessibility_error"].is_null(),
        "{}",
        state.data
    );
    assert!(!state.images.is_empty(), "WGC failed: {}", state.data);
    assert_eq!(&state.images[0].png[..8], b"\x89PNG\r\n\x1a\n");
    let elements = state.data["accessibility"]["elements"].as_array().unwrap();
    let edit = elements
        .iter()
        .find(|e| e["automation_id"] == "1001")
        .expect("editable control missing");
    let password = elements
        .iter()
        .find(|e| e["automation_id"] == "1002")
        .expect("password control missing");
    assert_eq!(password["password"], true);
    assert!(password["value"].is_null());
    assert!(!state.data.to_string().contains("fixture-secret"));
    let a = Action {
        action: "set_value".into(),
        window: Some(w),
        screenshot_id: state.data["screenshot_id"].as_str().map(str::to_owned),
        element_index: Some(edit["index"].as_u64().unwrap() as usize),
        text: Some("原生输入 verified".into()),
        ..Default::default()
    };
    assert!(
        send("other-session", Request::Perform(a.clone()))
            .await
            .is_err(),
        "snapshots must be session-scoped"
    );
    let result = send(
        "native-smoke",
        Request::PerformAndObserve {
            action: a.clone(),
            observe_after: true,
            include_text: true,
            include_screenshot: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.data["operation_completed"], true);
    let controls = result.data["observation"]["data"]["accessibility"]["elements"]
        .as_array()
        .unwrap();
    assert_eq!(
        controls
            .iter()
            .find(|e| e["automation_id"] == "1001")
            .unwrap()["value"],
        "原生输入 verified"
    );
    assert!(
        send("native-smoke", Request::Perform(a)).await.is_err(),
        "input must invalidate old snapshots"
    );
    let state = observe(w).await;
    let edit = state.data["accessibility"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["automation_id"] == "1001")
        .unwrap();
    let typed = Action {
        action: "type".into(),
        window: Some(w),
        screenshot_id: state.data["screenshot_id"].as_str().map(str::to_owned),
        element_index: Some(edit["index"].as_u64().unwrap() as usize),
        text: Some("中文🙂".into()),
        ..Default::default()
    };
    let typed = send(
        "native-smoke",
        Request::PerformAndObserve {
            action: typed,
            observe_after: true,
            include_text: true,
            include_screenshot: false,
        },
    )
    .await
    .unwrap();
    let controls = typed.data["observation"]["data"]["accessibility"]["elements"]
        .as_array()
        .unwrap();
    assert!(
        controls
            .iter()
            .find(|e| e["automation_id"] == "1001")
            .unwrap()["value"]
            .as_str()
            .unwrap()
            .contains("中文🙂"),
        "{}",
        typed.data
    );
    // A second fixture covers the first. WGC must still return the first window's pixels.
    {
        let overlay = fixture();
        let covered = observe(w).await;
        assert!(!covered.images.is_empty(), "{}", covered.data);
        drop(overlay);
    }
    let before = observe(w).await;
    unsafe {
        let _ = SetWindowPos(
            HWND(w.id as usize as *mut _),
            None,
            140,
            140,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER,
        );
    }
    let moved = Action {
        action: "click".into(),
        window: Some(w),
        screenshot_id: before.data["screenshot_id"].as_str().map(str::to_owned),
        x: Some(30),
        y: Some(60),
        ..Default::default()
    };
    let error = send("native-smoke", Request::Perform(moved))
        .await
        .unwrap_err();
    assert!(error.contains("moved"), "{error}");
    let bad = WindowTarget {
        pid: w.pid.wrapping_add(1),
        ..w
    };
    assert!(send(
        "native-smoke",
        Request::Observe {
            window: bad,
            include_text: true,
            include_screenshot: false
        }
    )
    .await
    .is_err());
    let closing = observe(w).await;
    let closed = send(
        "native-smoke",
        Request::PerformAndObserve {
            action: Action {
                action: "key".into(),
                window: Some(w),
                screenshot_id: closing.data["screenshot_id"].as_str().map(str::to_owned),
                keys: Some("alt+f4".into()),
                ..Default::default()
            },
            observe_after: true,
            include_text: true,
            include_screenshot: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(closed.data["operation_completed"], true, "{}", closed.data);
    assert_eq!(closed.data["goal_verified"], false);
    assert_eq!(
        closed.data["observation"]["success"], false,
        "closing the fixture must preserve input receipt when observation cannot find it"
    );
    println!("Native fixture: WGC pixels, UIA values/password redaction, Unicode set_value, window/session binding and stale-snapshot rejection passed");
}
