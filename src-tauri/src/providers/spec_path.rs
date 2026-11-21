//! 声明式协议 spec 的路径提取 mini-language
//!
//! 用于从响应 JSON 中提取文本 / 工具调用 / finish_reason / usage 等字段。
//! 语法（JSONPath 子集 + 管道后处理）：
//! - `$` 根节点
//! - `.a.b` 对象键遍历
//! - `[0]` 数组下标
//! - `[*]` 数组映射（对该路径上的每个元素继续求值，收集所有非空叶子）
//! - `[?(@.type=="thinking")]` 谓词过滤（`@` 指当前数组元素，支持 `==` 与 `!=`）
//! - `|join` / `|first` / `|trim` / `|self` 管道后处理
//!
//! 语义：
//! - 缺路径返回 `None`（不是错误），流式字段 None 时跳过即可
//! - 标量字段（content / finishReason / usage）默认 first 语义
//! - 数组映射（`[*]`）默认 join 语义（全部为字符串时拼接），除非显式给出后处理

use serde_json::Value;

/// 编译后的路径表达式，spec 加载时预编译，逐块求值是廉价遍历。
#[derive(Debug, Clone)]
pub struct PathExpr {
    /// 原始表达式（用于报错信息）
    pub raw: String,
    /// 分段
    segments: Vec<Segment>,
    /// 管道后处理
    post: Post,
}

#[derive(Debug, Clone)]
enum Segment {
    Key(String),
    Index(usize),
    Wildcard,
    Filter { key: String, op: CmpOp, value: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Post {
    Join,
    First,
    Trim,
    Self_,
}

impl PathExpr {
    /// 解析路径表达式。语法非法时返回 Err。
    pub fn parse(raw: &str) -> Result<Self, String> {
        let (path_part, post) = match raw.split_once('|') {
            Some((p, post_raw)) => {
                let post = match post_raw.trim() {
                    "join" => Post::Join,
                    "first" => Post::First,
                    "trim" => Post::Trim,
                    "self" => Post::Self_,
                    other => return Err(format!("未知后处理: `{other}`")),
                };
                (p, post)
            }
            None => (raw, Post::First),
        };

        let mut segments = Vec::new();
        let mut rest = path_part.trim();
        if let Some(stripped) = rest.strip_prefix('$') {
            rest = stripped;
        }
        // 形如 .a.b[0][*][?(@.x=="y")]
        let mut chars = rest.chars().peekable();
        let mut key_buf = String::new();
        let mut expect_key = false;

        while let Some(c) = chars.next() {
            match c {
                '.' => {
                    if !key_buf.is_empty() {
                        segments.push(Segment::Key(std::mem::take(&mut key_buf)));
                    }
                    expect_key = true;
                }
                '[' => {
                    if !key_buf.is_empty() {
                        segments.push(Segment::Key(std::mem::take(&mut key_buf)));
                    }
                    // 读取括号内容
                    let mut inner = String::new();
                    let mut closed = false;
                    for ic in chars.by_ref() {
                        if ic == ']' {
                            closed = true;
                            break;
                        }
                        inner.push(ic);
                    }
                    if !closed {
                        return Err("路径数组段缺少 `]`".to_string());
                    }
                    let inner = inner.trim();
                    if inner == "*" {
                        segments.push(Segment::Wildcard);
                    } else if let Some(filter) = parse_filter(inner)? {
                        segments.push(filter);
                    } else if let Ok(idx) = inner.parse::<usize>() {
                        segments.push(Segment::Index(idx));
                    } else {
                        return Err(format!("无法解析路径段 `[{inner}]`"));
                    }
                    expect_key = false;
                }
                _ => {
                    key_buf.push(c);
                    expect_key = false;
                }
            }
        }
        if !key_buf.is_empty() {
            segments.push(Segment::Key(key_buf));
        }
        if expect_key {
            return Err("路径不能以 `.` 结尾".to_string());
        }

        Ok(PathExpr {
            raw: raw.to_string(),
            segments,
            post,
        })
    }

    /// 求值：返回 `Vec`（1 个或多个叶子）。
    ///
    /// 标量字段应调用 [`PathExpr::first_str`]；数组映射字段应调用
    /// [`PathExpr::evaluate`] 后按 post 合并。
    pub fn evaluate(&self, root: &Value) -> Vec<Value> {
        let mut results = Vec::new();
        self.walk(root, &self.segments, &mut results);
        results
    }

    /// 求值并返回字符串（first / join 语义）。
    pub fn first_str(&self, root: &Value) -> Option<String> {
        let vals = self.evaluate(root);
        match self.post {
            Post::First => vals
                .into_iter()
                .find(|v| !v.is_null())
                .and_then(|v| v.as_str().map(String::from)),
            Post::Join => {
                let parts: Vec<String> = vals
                    .into_iter()
                    .filter(|v| !v.is_null())
                    .filter_map(|v| match v {
                        Value::String(s) => Some(s),
                        Value::Number(n) => Some(n.to_string()),
                        Value::Bool(b) => Some(b.to_string()),
                        _ => None,
                    })
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.concat())
                }
            }
            Post::Trim => vals
                .into_iter()
                .find(|v| !v.is_null())
                .and_then(|v| v.as_str().map(|s| s.trim().to_string())),
            Post::Self_ => vals
                .into_iter()
                .find(|v| !v.is_null())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                }),
        }
    }

    /// 求值并返回第一个非 null 的原始 Value（用于 tool 参数等对象字段）。
    pub fn first_value(&self, root: &Value) -> Option<Value> {
        self.evaluate(root)
            .into_iter()
            .find(|v| !v.is_null())
    }

    fn walk(&self, node: &Value, segs: &[Segment], out: &mut Vec<Value>) {
        let Some((head, tail)) = segs.split_first() else {
            out.push(node.clone());
            return;
        };
        match head {
            Segment::Key(k) => match node.get(k) {
                Some(child) => self.walk(child, tail, out),
                None => {}
            },
            Segment::Index(i) => match node.get(*i) {
                Some(child) => self.walk(child, tail, out),
                None => {}
            },
            Segment::Wildcard => match node {
                Value::Array(arr) => {
                    for item in arr {
                        self.walk(item, tail, out);
                    }
                }
                _ => {}
            },
            Segment::Filter { key, op, value } => match node {
                Value::Array(arr) => {
                    for item in arr {
                        let matched = item
                            .get(key)
                            .and_then(|v| v.as_str())
                            .map(|s| match op {
                                CmpOp::Eq => s == value,
                                CmpOp::Ne => s != value,
                            })
                            .unwrap_or(match op {
                                CmpOp::Eq => false,
                                CmpOp::Ne => true,
                            });
                        if matched {
                            self.walk(item, tail, out);
                        }
                    }
                }
                _ => {}
            },
        }
    }
}

/// 解析谓词过滤 `?(@.key=="value")` / `?(@.key!="value")`
fn parse_filter(inner: &str) -> Result<Option<Segment>, String> {
    let inner = inner.trim();
    let Some(body) = inner.strip_prefix("?(").and_then(|s| s.strip_suffix(')')) else {
        return Ok(None);
    };
    let body = body.trim();
    let (key, op, value) = if let Some((l, r)) = body.split_once("==") {
        (l.trim(), CmpOp::Eq, r.trim())
    } else if let Some((l, r)) = body.split_once("!=") {
        (l.trim(), CmpOp::Ne, r.trim())
    } else {
        return Err(format!("无法解析过滤条件 `{body}`"));
    };
    if value.is_empty() {
        return Err(format!("过滤条件缺少比较值 `{body}`"));
    }
    let key = key.strip_prefix("@.").unwrap_or(key).to_string();
    let value = value.trim_matches('"').trim_matches('\'').to_string();
    if key.is_empty() {
        return Err(format!("过滤条件缺少字段名 `{body}`"));
    }
    Ok(Some(Segment::Filter { key, op, value }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn expr(raw: &str) -> PathExpr {
        PathExpr::parse(raw).unwrap_or_else(|e| panic!("parse `{raw}` failed: {e}"))
    }

    #[test]
    fn scalar_path() {
        let v = json!({"choices": [{"message": {"content": "hi"}}]});
        assert_eq!(
            expr("$.choices[0].message.content").first_str(&v),
            Some("hi".to_string())
        );
    }

    #[test]
    fn wildcard_join() {
        let v = json!({"candidates": [{"content": {"parts": [{"text": "a"}, {"text": "b"}]}}]});
        assert_eq!(
            expr("$.candidates[0].content.parts[*].text|join").first_str(&v),
            Some("ab".to_string())
        );
    }

    #[test]
    fn filter_thinking_blocks() {
        let v = json!({"content": [
            {"type": "thinking", "thinking": "secret"},
            {"type": "text", "text": "visible"}
        ]});
        assert_eq!(
            expr("$.content[?(@.type==\"thinking\")].thinking|join").first_str(&v),
            Some("secret".to_string())
        );
        assert_eq!(
            expr("$.content[?(@.type==\"text\")].text|join").first_str(&v),
            Some("visible".to_string())
        );
    }

    #[test]
    fn filter_ne() {
        let v = json!({"output": [
            {"type": "message", "name": "a"},
            {"type": "function_call", "name": "b"}
        ]});
        assert_eq!(
            expr("$.output[?(@.type!=\"function_call\")].name|first").first_str(&v),
            Some("a".to_string())
        );
    }

    #[test]
    fn missing_path_returns_none() {
        let v = json!({"a": 1});
        assert_eq!(expr("$.b.c").first_str(&v), None);
        assert!(expr("$.choices[0].delta.reasoning_content").first_str(&v).is_none());
    }

    #[test]
    fn tool_name_self() {
        let v = json!({"candidates": [{"content": {"parts": [
            {"functionCall": {"name": "get_weather", "args": {}}}
        ]}}]});
        assert_eq!(
            expr("$.candidates[0].content.parts[*].functionCall.name|self").first_str(&v),
            Some("get_weather".to_string())
        );
    }

    #[test]
    fn invalid_syntax_errors() {
        assert!(PathExpr::parse("$.a[?(@.x==)]").is_err());
        assert!(PathExpr::parse("$.a|bogus").is_err());
        assert!(PathExpr::parse("$.[").is_err());
    }
}
