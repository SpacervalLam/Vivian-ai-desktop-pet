//! Remove model tool protocol from user-visible text, including split stream tokens.
#[derive(Default)]
pub struct ProtocolTextFilter {
    pending: String,
    blocked: bool,
}

impl ProtocolTextFilter {
    pub fn push(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        let mut output = String::new();
        loop {
            if self.blocked {
                // Suppress parameters as well as tags until the outer calls block closes.
                let Some(start) = self.pending.find("</") else {
                    let partial = self.pending.ends_with('<');
                    self.pending.clear();
                    if partial { self.pending.push('<'); }
                    break;
                };
                if start > 0 { self.pending.drain(..start); }
                let Some(end) = self.pending.find('>') else { break; };
                let tag = self.pending[..=end].to_string();
                self.pending.drain(..=end);
                if tag.contains("DSML") && tag.contains("calls") { self.blocked = false; }
                continue;
            }
            let Some(start) = self.pending.find('<') else {
                output.push_str(&self.pending);
                self.pending.clear();
                break;
            };
            output.push_str(&self.pending[..start]);
            self.pending.drain(..start);
            let Some(end) = self.pending.find('>') else { break; };
            let tag = self.pending[..=end].to_string();
            self.pending.drain(..=end);
            if tag.contains("DSML") {
                self.blocked = !tag.starts_with("</");
            } else {
                output.push_str(&tag);
            }
        }
        output
    }
}

pub fn strip_protocol_text(text: &str) -> String {
    let mut filter = ProtocolTextFilter::default();
    let mut clean = filter.push(text);
    // Preserve incomplete ordinary text, but never incomplete protocol tags.
    if !filter.blocked && !filter.pending.contains("DSML") {
        clean.push_str(&filter.pending);
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_blocks_and_parameters_are_hidden() {
        let raw = "先看看。<｜｜DSML｜｜ calls>\n<｜｜DSML｜｜ invoke name=\"wallpaper_list\"><｜｜DSML｜｜ parameter>天气之子</｜｜DSML｜｜ parameter></｜｜DSML｜｜ invoke></｜｜DSML｜｜ calls>稍后再试。";
        assert_eq!(strip_protocol_text(raw), "先看看。稍后再试。");
        for size in 1..raw.chars().count() {
            let mut filter = ProtocolTextFilter::default();
            let chars: Vec<_> = raw.chars().collect();
            let mut out = String::new();
            for chunk in chars.chunks(size) { out.push_str(&filter.push(&chunk.iter().collect::<String>())); }
            assert_eq!(out, "先看看。稍后再试。", "chunk size {size}");
        }
    }
    #[test]
    fn incomplete_protocol_is_hidden_and_ordinary_text_survives() {
        assert_eq!(strip_protocol_text("好<｜｜DSML｜｜ invoke>secret"), "好");
        assert_eq!(strip_protocol_text("好<｜｜DSML"), "好");
        assert_eq!(strip_protocol_text("a < b"), "a < b");
        assert_eq!(strip_protocol_text("<b>普通内容</b>"), "<b>普通内容</b>");
    }
}
