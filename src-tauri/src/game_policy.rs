//! Pure game recognition rules. A launcher, browser or video title is not a game.
pub fn normalize_process(process: &str) -> String {
    let name = process.trim().rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_owned()
}

pub fn same_process(a: &str, b: &str) -> bool {
    let a = normalize_process(a);
    !a.is_empty() && a == normalize_process(b)
}

pub fn excluded(process: &str) -> bool {
    matches!(normalize_process(process).as_str(),
        "steam" | "steamwebhelper" | "epicgameslauncher" | "epic" | "battle.net" | "battlenet"
        | "origin" | "ea" | "eaapp" | "eadesktop" | "galaxy" | "gog" | "riotclientservices"
        | "leagueclient" | "leagueclientux" | "minecraftlauncher" | "ubisoftconnect" | "upc"
        | "chrome" | "msedge" | "firefox" | "brave" | "opera"
        | "vivaldi" | "arc" | "explorer" | "applicationframehost" | "code" | "cursor"
        | "vlc" | "potplayer" | "mpv" | "obs64" | "obs32" | "taskmgr")
}

/// Shared runtimes also host editors and ordinary apps: reclassify each window title.
pub fn shared_host(process: &str) -> bool {
    matches!(normalize_process(process).as_str(), "java" | "javaw" | "python" | "python3"
        | "pythonw" | "wine" | "wine64" | "dotnet" | "node" | "electron")
}

pub fn known_game(process: &str, learned: &[String]) -> bool {
    if excluded(process) || shared_host(process) { return false; }
    learned.iter().any(|p| same_process(p, process)) || matches!(normalize_process(process).as_str(),
        "league of legends" | "genshinimpact" | "yuan shen" | "yuanshen" | "starrail"
        | "zenlesszonezero" | "minecraft" | "valorant" | "valorant-win64-shipping"
        | "cs2" | "dota2" | "eldenring" | "stardew valley" | "terraria" | "overwatch")
}

/// Strict model protocol; uncertainty never grows the persistent process table.
pub fn confirmed_game(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) else { return false; };
    value["is_game"].as_bool() == Some(true)
        && value["confidence"].as_f64().is_some_and(|confidence| (0.9..=1.0).contains(&confidence))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_match_with_or_without_extension_but_not_substrings() {
        assert!(same_process("League of Legends.exe", "league of legends"));
        assert!(same_process("C:\\Games\\GAME.EXE", "game"));
        assert!(!same_process("game.exe.bak", "game.exe"));
        assert!(!same_process("", ""));
    }
    #[test]
    fn game_content_in_other_apps_does_not_enable_game_mode() {
        for app in ["steam.exe", "chrome.exe", "Code", "vlc", "LeagueClientUx"] {
            assert!(excluded(app));
            assert!(!known_game(app, &[app.into()]));
        }
        assert!(known_game("GenshinImpact", &[]));
        assert!(known_game("newgame", &["NewGame.exe".into()]));
        assert!(!known_game("newgame-helper", &["NewGame.exe".into()]));
        assert!(!known_game("javaw", &["javaw.exe".into()]), "Minecraft's runtime must not identify every Java app as a game");
    }
    #[test]
    fn uncertain_or_malformed_classification_is_never_learned() {
        assert!(confirmed_game(r#"{"is_game":true,"confidence":0.95}"#));
        for raw in [r#"{"is_game":false,"confidence":1}"#, r#"{"is_game":true,"confidence":0.7}"#,
            r#"{"is_game":true,"confidence":2}"#, "true", "not json"] {
            assert!(!confirmed_game(raw));
        }
    }
}
