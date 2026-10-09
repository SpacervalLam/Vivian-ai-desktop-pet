pub fn normalize(value: &str) -> String {
    value.split('+').map(|part| match part { "LeftAlt" | "RightAlt" => "Alt", _ => part }).collect::<Vec<_>>().join("+")
}
pub fn matches_alt(value: &str, left: bool, right: bool) -> bool {
    (!value.split('+').any(|p| p == "LeftAlt") || (left && !right))
        && (!value.split('+').any(|p| p == "RightAlt") || (right && !left))
}
pub fn physical_modifiers_match(value: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LMENU, VK_RMENU};
        unsafe { matches_alt(value, GetAsyncKeyState(VK_LMENU.0 as i32) < 0, GetAsyncKeyState(VK_RMENU.0 as i32) < 0) }
    }
    #[cfg(not(target_os = "windows"))]
    { let _ = value; true }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn sided_alt() {
        assert_eq!(normalize("Control+LeftAlt+A"), "Control+Alt+A");
        assert!(matches_alt("Control+LeftAlt+A", true, false));
        assert!(!matches_alt("Control+LeftAlt+A", false, true));
        assert!(!matches_alt("Control+LeftAlt+A", true, true));
        assert!(matches_alt("Control+Alt+A", false, true));
    }
}
