//! 「这句话是不是在拒绝 / 只是在假设」——请求升级前的语法闸门。
//!
//! 否定与假设是封闭的语法类，按子句开头判定即可，不需要列举
//! 「别帮我 / 不用给我 / 不要查」这种组合。拆句后只要还有一个不是在拒绝
//! 或假设的子句，就仍按请求处理（"别查了，帮我关掉音乐" 不该被整句否定）。
//!
//! 零依赖，便于隔离验证。

/// 子句由此切开：中英标点与换行。
const CLAUSE_SEPARATORS: &[char] = &['，', '。', '！', '？', ',', '.', '!', '?', '；', ';', '\n'];

/// 子句开头出现即表示「不要求动手」的标记。
const WITHHOLDING: &[&str] = &[
    // 否定
    "别", "不要", "不用", "不必", "无需", "没必要", "先不", "不想", "甭", "勿",
    "don't", "do not", "no need", "without",
    // 假设
    "如果", "假如", "要是", "假设", "万一", "若", "if ", "whether", "would",
];

/// 整句都在拒绝或假设时为真。
pub(crate) fn withheld(input: &str) -> bool {
    let clauses: Vec<&str> = input
        .split(CLAUSE_SEPARATORS)
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .collect();
    !clauses.is_empty() && clauses.iter().all(|clause| starts_with_withholding(clause))
}

fn starts_with_withholding(clause: &str) -> bool {
    let lower = clause.to_lowercase();
    WITHHOLDING.iter().any(|marker| lower.starts_with(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_refusals_are_withheld() {
        for input in ["别帮我查这个", "不用帮我做", "不要换成这张", "别找了", "不想设置壁纸"] {
            assert!(withheld(input), "{input}");
        }
    }

    #[test]
    fn hypotheticals_are_withheld() {
        for input in ["如果换了会怎样", "假如帮我做一份呢", "要是能查到就好了"] {
            assert!(withheld(input), "{input}");
        }
    }

    #[test]
    fn one_live_clause_keeps_the_request() {
        for input in ["别查了，帮我关掉音乐", "不用解释。帮我把壁纸换成那张", "如果不行就算了，帮我搜一下"] {
            assert!(!withheld(input), "{input}");
        }
    }

    #[test]
    fn ordinary_requests_and_negation_inside_words_are_not_withheld() {
        for input in ["帮我查一下星芯科技", "帮我找家不错的餐厅", "换个壁纸", "搜索一下相关新闻"] {
            assert!(!withheld(input), "{input}");
        }
        // "不错" 里的 "不" 不在子句开头，不能被当否定。
        assert!(!withheld("不错啊，帮我查查"));
    }

    #[test]
    fn empty_input_is_not_withheld() {
        assert!(!withheld("   "));
    }
}
