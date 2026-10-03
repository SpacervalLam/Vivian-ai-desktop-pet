//! Packaging does not confer execution capability on instruction documents.
pub fn contribution_kind(executable: bool, data: bool, guidance: bool) -> &'static str {
    match (executable, data, guidance) {
        (true, _, true) => "mixed",
        (true, _, false) => "executable",
        (false, true, _) => "data",
        (false, false, true) => "guidance",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guidance_does_not_become_executable_when_bundled_with_data() {
        assert_eq!(contribution_kind(false, false, true), "guidance");
        assert_eq!(contribution_kind(false, true, true), "data");
        assert_eq!(contribution_kind(true, false, true), "mixed");
        assert_eq!(contribution_kind(true, true, false), "executable");
    }
}
