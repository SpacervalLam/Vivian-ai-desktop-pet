//! Bounded, browser-native keyboard input. No page-provided JavaScript or CDP methods.
use serde_json::{json, Value};

pub(crate) fn key_events(chord: &str) -> Result<(Value, Value), String> {
    let parts: Vec<_> = chord.split('+').collect();
    let (last, modifiers) = parts.split_last().ok_or("A key is required")?;
    let mut mask = 0;
    for modifier in modifiers {
        let bit = match *modifier {
            "Alt" => 1,
            "Control" | "Ctrl" => 2,
            "Meta" => 4,
            "Shift" => 8,
            _ => return Err("Unknown keyboard modifier".into()),
        };
        if mask & bit != 0 {
            return Err("Duplicate keyboard modifier".into());
        }
        mask |= bit;
    }
    let (key, code, vk) = match *last {
        "Enter" => ("Enter".into(), "Enter".into(), 13),
        "Tab" => ("Tab".into(), "Tab".into(), 9),
        "Escape" | "Esc" => ("Escape".into(), "Escape".into(), 27),
        "Backspace" => ("Backspace".into(), "Backspace".into(), 8),
        "Delete" => ("Delete".into(), "Delete".into(), 46),
        "Home" => ("Home".into(), "Home".into(), 36),
        "End" => ("End".into(), "End".into(), 35),
        "PageUp" => ("PageUp".into(), "PageUp".into(), 33),
        "PageDown" => ("PageDown".into(), "PageDown".into(), 34),
        "ArrowLeft" => ("ArrowLeft".into(), "ArrowLeft".into(), 37),
        "ArrowUp" => ("ArrowUp".into(), "ArrowUp".into(), 38),
        "ArrowRight" => ("ArrowRight".into(), "ArrowRight".into(), 39),
        "ArrowDown" => ("ArrowDown".into(), "ArrowDown".into(), 40),
        " " | "Space" => (" ".into(), "Space".into(), 32),
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphanumeric() => {
            let c = value.as_bytes()[0];
            let key = if mask & 8 != 0 {
                value.to_ascii_uppercase()
            } else {
                value.to_ascii_lowercase()
            };
            (
                key,
                format!(
                    "{}{}",
                    if c.is_ascii_digit() { "Digit" } else { "Key" },
                    value.to_ascii_uppercase()
                ),
                c.to_ascii_uppercase() as i32,
            )
        }
        _ => return Err("Unsupported key; use browser_type for text and Unicode".into()),
    };
    let mut down = json!({"type":"keyDown","key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":vk,"modifiers":mask});
    // Shortcut chords must never insert their printable character.
    if mask & 7 == 0 {
        let text = if vk == 13 {
            "\r"
        } else if key.len() == 1 {
            &key
        } else {
            ""
        };
        if !text.is_empty() {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
    }
    let up = json!({"type":"keyUp","key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":vk,"modifiers":mask});
    Ok((down, up))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts_do_not_type_and_release_the_same_key() {
        let (down, up) = key_events("Control+Shift+ArrowLeft").unwrap();
        assert_eq!(down["modifiers"], 10);
        assert!(down.get("text").is_none());
        assert_eq!(down["code"], up["code"]);
        assert_eq!(key_events("Ctrl+A").unwrap().0["code"], "KeyA");
        assert_eq!(key_events("Enter").unwrap().0["text"], "\r");
        assert_eq!(key_events("Shift+z").unwrap().0["text"], "Z");
        for invalid in ["", "Ctrl+", "Ctrl+Ctrl+A", "Super+A", "中文", "F99", "A+B"] {
            assert!(key_events(invalid).is_err(), "{invalid}");
        }
    }
}
