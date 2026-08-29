//! 用户活动观察语料库 —— 为前台窗口/标题的快速嵌入匹配提供丰富种子。
//!
//! 对齐 live2D 表情的语料库思想（`emotion::embedding_corpus_*`）：预先编写大量
//! 典型窗口标题/页面/短语，每条标注细粒度活动标签，由 `ActivityEmbeddingClassifier`
//! 将语料嵌入后，对查询文本做 Top-K softmax 投票，得到最可能的用户当前活动。
//!
//! 标签体系为细粒度（比进程名粗分类更具体），例如「写代码/调试排查/终端命令/
//! 浏览网页/网上购物/看视频/听音乐/玩游戏/聊天/处理邮件/写文档/表格处理/
//! 演示文稿/记笔记/图片设计/视频剪辑/阅读/学习/文件管理/系统设置」。
//!
//! 设计原则：
//! - 中文为主，兼顾英文常见标题/域名片段
//! - 每条覆盖一个可辨识的窗口标题、应用名或页面语境
//! - 同一活动标签下尽量多样化，提升泛化能力

/// 活动语料条目 —— 一条可辨识文本 + 它的细粒度活动标签
#[derive(Debug, Clone, Copy)]
pub struct ActivityCorpusEntry {
    /// 典型窗口标题 / 应用名 / 页面语境
    pub text: &'static str,
    /// 细粒度活动标签（如「写代码」「看视频」）
    pub activity: &'static str,
}

/// 全部活动语料（扁平列表，按活动标签分组便于维护）
pub static ACTIVITY_CORPUS: &[ActivityCorpusEntry] = &[
    // ──────────────── 写代码 ────────────────
    ActivityCorpusEntry { text: "main.rs - VS Code", activity: "写代码" },
    ActivityCorpusEntry { text: "index.js - IntelliJ IDEA", activity: "写代码" },
    ActivityCorpusEntry { text: "src/ - Visual Studio Code", activity: "写代码" },
    ActivityCorpusEntry { text: "app.tsx - cursor", activity: "写代码" },
    ActivityCorpusEntry { text: "untitled-1.py - PyCharm", activity: "写代码" },
    ActivityCorpusEntry { text: "Rust 项目 - RustRover", activity: "写代码" },
    ActivityCorpusEntry { text: "webstorm - app.js", activity: "写代码" },
    ActivityCorpusEntry { text: "main.c - Vim", activity: "写代码" },
    ActivityCorpusEntry { text: "lib.rs - Neovim", activity: "写代码" },
    ActivityCorpusEntry { text: "main.py - Sublime Text", activity: "写代码" },
    ActivityCorpusEntry { text: "android studio - MainActivity.kt", activity: "写代码" },
    ActivityCorpusEntry { text: "xcode - ViewController.swift", activity: "写代码" },
    ActivityCorpusEntry { text: "namespace Foo", activity: "写代码" },
    ActivityCorpusEntry { text: "fn main", activity: "写代码" },
    ActivityCorpusEntry { text: "import React", activity: "写代码" },
    ActivityCorpusEntry { text: "cargo build", activity: "写代码" },
    ActivityCorpusEntry { text: "git commit", activity: "写代码" },
    ActivityCorpusEntry { text: "git push origin main", activity: "写代码" },
    ActivityCorpusEntry { text: "PR #123", activity: "写代码" },
    ActivityCorpusEntry { text: "pull request", activity: "写代码" },

    // ──────────────── 调试排查 ────────────────
    ActivityCorpusEntry { text: "Debug Console", activity: "调试排查" },
    ActivityCorpusEntry { text: "调试控制台", activity: "调试排查" },
    ActivityCorpusEntry { text: "断点", activity: "调试排查" },
    ActivityCorpusEntry { text: "breakpoints", activity: "调试排查" },
    ActivityCorpusEntry { text: "call stack", activity: "调试排查" },
    ActivityCorpusEntry { text: "调用堆栈", activity: "调试排查" },
    ActivityCorpusEntry { text: "watch window", activity: "调试排查" },
    ActivityCorpusEntry { text: "监视窗口", activity: "调试排查" },
    ActivityCorpusEntry { text: "stack trace", activity: "调试排查" },
    ActivityCorpusEntry { text: "error: expected", activity: "调试排查" },
    ActivityCorpusEntry { text: "exception thrown", activity: "调试排查" },
    ActivityCorpusEntry { text: "崩溃日志", activity: "调试排查" },
    ActivityCorpusEntry { text: "panic", activity: "调试排查" },
    ActivityCorpusEntry { text: "debugger", activity: "调试排查" },
    ActivityCorpusEntry { text: "调试运行", activity: "调试排查" },

    // ──────────────── 终端命令 ────────────────
    ActivityCorpusEntry { text: "Administrator: Windows PowerShell", activity: "终端命令" },
    ActivityCorpusEntry { text: "Windows Terminal", activity: "终端命令" },
    ActivityCorpusEntry { text: "Terminal", activity: "终端命令" },
    ActivityCorpusEntry { text: "cmd.exe", activity: "终端命令" },
    ActivityCorpusEntry { text: "Git Bash", activity: "终端命令" },
    ActivityCorpusEntry { text: "bash", activity: "终端命令" },
    ActivityCorpusEntry { text: "zsh", activity: "终端命令" },
    ActivityCorpusEntry { text: "npm run dev", activity: "终端命令" },
    ActivityCorpusEntry { text: "pip install", activity: "终端命令" },
    ActivityCorpusEntry { text: "ssh connection", activity: "终端命令" },
    ActivityCorpusEntry { text: "docker ps", activity: "终端命令" },
    ActivityCorpusEntry { text: "kubectl", activity: "终端命令" },
    ActivityCorpusEntry { text: "ls -la", activity: "终端命令" },
    ActivityCorpusEntry { text: "top", activity: "终端命令" },
    ActivityCorpusEntry { text: "htop", activity: "终端命令" },

    // ──────────────── 浏览网页 ────────────────
    ActivityCorpusEntry { text: "Google Chrome", activity: "浏览网页" },
    ActivityCorpusEntry { text: "Mozilla Firefox", activity: "浏览网页" },
    ActivityCorpusEntry { text: "Microsoft Edge", activity: "浏览网页" },
    ActivityCorpusEntry { text: "New Tab", activity: "浏览网页" },
    ActivityCorpusEntry { text: "百度一下", activity: "浏览网页" },
    ActivityCorpusEntry { text: "GitHub", activity: "浏览网页" },
    ActivityCorpusEntry { text: "知乎", activity: "浏览网页" },
    ActivityCorpusEntry { text: "豆瓣", activity: "浏览网页" },
    ActivityCorpusEntry { text: "微博", activity: "浏览网页" },
    ActivityCorpusEntry { text: "Reddit", activity: "浏览网页" },
    ActivityCorpusEntry { text: "推特", activity: "浏览网页" },
    ActivityCorpusEntry { text: "X / Twitter", activity: "浏览网页" },
    ActivityCorpusEntry { text: "bookmarks", activity: "浏览网页" },
    ActivityCorpusEntry { text: "主页", activity: "浏览网页" },

    // ──────────────── 搜索资料 ────────────────
    ActivityCorpusEntry { text: "Google 搜索", activity: "搜索资料" },
    ActivityCorpusEntry { text: "百度搜索", activity: "搜索资料" },
    ActivityCorpusEntry { text: "搜索", activity: "搜索资料" },
    ActivityCorpusEntry { text: "搜索结果", activity: "搜索资料" },
    ActivityCorpusEntry { text: "Stack Overflow", activity: "搜索资料" },
    ActivityCorpusEntry { text: "MDN", activity: "搜索资料" },
    ActivityCorpusEntry { text: "文档", activity: "搜索资料" },
    ActivityCorpusEntry { text: "资料", activity: "搜索资料" },
    ActivityCorpusEntry { text: "如何实现", activity: "搜索资料" },
    ActivityCorpusEntry { text: "教程", activity: "搜索资料" },
    ActivityCorpusEntry { text: "reference", activity: "搜索资料" },
    ActivityCorpusEntry { text: "API 文档", activity: "搜索资料" },

    // ──────────────── 网上购物 ────────────────
    ActivityCorpusEntry { text: "淘宝", activity: "网上购物" },
    ActivityCorpusEntry { text: "淘宝网", activity: "网上购物" },
    ActivityCorpusEntry { text: "京东", activity: "网上购物" },
    ActivityCorpusEntry { text: "天猫", activity: "网上购物" },
    ActivityCorpusEntry { text: "拼多多", activity: "网上购物" },
    ActivityCorpusEntry { text: "唯品会", activity: "网上购物" },
    ActivityCorpusEntry { text: "购物车", activity: "网上购物" },
    ActivityCorpusEntry { text: "商品详情", activity: "网上购物" },
    ActivityCorpusEntry { text: "下单", activity: "网上购物" },
    ActivityCorpusEntry { text: "亚马逊", activity: "网上购物" },
    ActivityCorpusEntry { text: "Amazon", activity: "网上购物" },
    ActivityCorpusEntry { text: "订单", activity: "网上购物" },

    // ──────────────── 看视频 ────────────────
    ActivityCorpusEntry { text: "哔哩哔哩", activity: "看视频" },
    ActivityCorpusEntry { text: "bilibili", activity: "看视频" },
    ActivityCorpusEntry { text: "YouTube", activity: "看视频" },
    ActivityCorpusEntry { text: "Netflix", activity: "看视频" },
    ActivityCorpusEntry { text: "腾讯视频", activity: "看视频" },
    ActivityCorpusEntry { text: "爱奇艺", activity: "看视频" },
    ActivityCorpusEntry { text: "优酷", activity: "看视频" },
    ActivityCorpusEntry { text: "抖音", activity: "看视频" },
    ActivityCorpusEntry { text: "抖音短视频", activity: "看视频" },
    ActivityCorpusEntry { text: "PotPlayer", activity: "看视频" },
    ActivityCorpusEntry { text: "VLC media player", activity: "看视频" },
    ActivityCorpusEntry { text: "mpv", activity: "看视频" },
    ActivityCorpusEntry { text: "视频", activity: "看视频" },
    ActivityCorpusEntry { text: "电影", activity: "看视频" },
    ActivityCorpusEntry { text: "剧集", activity: "看视频" },

    // ──────────────── 听音乐 ────────────────
    ActivityCorpusEntry { text: "网易云音乐", activity: "听音乐" },
    ActivityCorpusEntry { text: "QQ音乐", activity: "听音乐" },
    ActivityCorpusEntry { text: "酷狗音乐", activity: "听音乐" },
    ActivityCorpusEntry { text: "Spotify", activity: "听音乐" },
    ActivityCorpusEntry { text: "Apple Music", activity: "听音乐" },
    ActivityCorpusEntry { text: "foobar2000", activity: "听音乐" },
    ActivityCorpusEntry { text: "播放列表", activity: "听音乐" },
    ActivityCorpusEntry { text: "歌单", activity: "听音乐" },
    ActivityCorpusEntry { text: "音乐", activity: "听音乐" },
    ActivityCorpusEntry { text: "播客", activity: "听音乐" },
    ActivityCorpusEntry { text: "podcast", activity: "听音乐" },

    // ──────────────── 玩游戏 ────────────────
    ActivityCorpusEntry { text: "Steam", activity: "玩游戏" },
    ActivityCorpusEntry { text: "Minecraft", activity: "玩游戏" },
    ActivityCorpusEntry { text: "原神", activity: "玩游戏" },
    ActivityCorpusEntry { text: "英雄联盟", activity: "玩游戏" },
    ActivityCorpusEntry { text: "League of Legends", activity: "玩游戏" },
    ActivityCorpusEntry { text: "CS:GO", activity: "玩游戏" },
    ActivityCorpusEntry { text: "Counter-Strike", activity: "玩游戏" },
    ActivityCorpusEntry { text: "荒野大镖客", activity: "玩游戏" },
    ActivityCorpusEntry { text: "Epic Games", activity: "玩游戏" },
    ActivityCorpusEntry { text: "游戏", activity: "玩游戏" },
    ActivityCorpusEntry { text: "网游", activity: "玩游戏" },
    ActivityCorpusEntry { text: "单机游戏", activity: "玩游戏" },

    // ──────────────── 聊天 ────────────────
    ActivityCorpusEntry { text: "微信", activity: "聊天" },
    ActivityCorpusEntry { text: "WeChat", activity: "聊天" },
    ActivityCorpusEntry { text: "QQ", activity: "聊天" },
    ActivityCorpusEntry { text: "Discord", activity: "聊天" },
    ActivityCorpusEntry { text: "Telegram", activity: "聊天" },
    ActivityCorpusEntry { text: "Slack", activity: "聊天" },
    ActivityCorpusEntry { text: "钉钉", activity: "聊天" },
    ActivityCorpusEntry { text: "飞书", activity: "聊天" },
    ActivityCorpusEntry { text: "群聊", activity: "聊天" },
    ActivityCorpusEntry { text: "消息", activity: "聊天" },
    ActivityCorpusEntry { text: "聊天", activity: "聊天" },
    ActivityCorpusEntry { text: "WhatsApp", activity: "聊天" },
    ActivityCorpusEntry { text: "Line", activity: "聊天" },

    // ──────────────── 处理邮件 ────────────────
    ActivityCorpusEntry { text: "Outlook", activity: "处理邮件" },
    ActivityCorpusEntry { text: "Foxmail", activity: "处理邮件" },
    ActivityCorpusEntry { text: "Thunderbird", activity: "处理邮件" },
    ActivityCorpusEntry { text: "Gmail", activity: "处理邮件" },
    ActivityCorpusEntry { text: "收件箱", activity: "处理邮件" },
    ActivityCorpusEntry { text: "inbox", activity: "处理邮件" },
    ActivityCorpusEntry { text: "邮件", activity: "处理邮件" },
    ActivityCorpusEntry { text: "写邮件", activity: "处理邮件" },
    ActivityCorpusEntry { text: "草稿", activity: "处理邮件" },
    ActivityCorpusEntry { text: "已发送", activity: "处理邮件" },

    // ──────────────── 写文档 ────────────────
    ActivityCorpusEntry { text: "Microsoft Word", activity: "写文档" },
    ActivityCorpusEntry { text: "WPS 文字", activity: "写文档" },
    ActivityCorpusEntry { text: "Google Docs", activity: "写文档" },
    ActivityCorpusEntry { text: "LibreOffice Writer", activity: "写文档" },
    ActivityCorpusEntry { text: "word 文档", activity: "写文档" },
    ActivityCorpusEntry { text: "docx", activity: "写文档" },
    ActivityCorpusEntry { text: "论文", activity: "写文档" },
    ActivityCorpusEntry { text: "报告", activity: "写文档" },
    ActivityCorpusEntry { text: "文档正文", activity: "写文档" },

    // ──────────────── 表格处理 ────────────────
    ActivityCorpusEntry { text: "Microsoft Excel", activity: "表格处理" },
    ActivityCorpusEntry { text: "WPS 表格", activity: "表格处理" },
    ActivityCorpusEntry { text: "Google Sheets", activity: "表格处理" },
    ActivityCorpusEntry { text: "数据透视表", activity: "表格处理" },
    ActivityCorpusEntry { text: "xlsx", activity: "表格处理" },
    ActivityCorpusEntry { text: "excel", activity: "表格处理" },
    ActivityCorpusEntry { text: "表格", activity: "表格处理" },
    ActivityCorpusEntry { text: "电子表格", activity: "表格处理" },
    ActivityCorpusEntry { text: "求和", activity: "表格处理" },

    // ──────────────── 演示文稿 ────────────────
    ActivityCorpusEntry { text: "Microsoft PowerPoint", activity: "演示文稿" },
    ActivityCorpusEntry { text: "WPS 演示", activity: "演示文稿" },
    ActivityCorpusEntry { text: "Google Slides", activity: "演示文稿" },
    ActivityCorpusEntry { text: "幻灯片", activity: "演示文稿" },
    ActivityCorpusEntry { text: "pptx", activity: "演示文稿" },
    ActivityCorpusEntry { text: "powerpoint", activity: "演示文稿" },
    ActivityCorpusEntry { text: "演示文稿", activity: "演示文稿" },

    // ──────────────── 记笔记 ────────────────
    ActivityCorpusEntry { text: "Obsidian", activity: "记笔记" },
    ActivityCorpusEntry { text: "Notion", activity: "记笔记" },
    ActivityCorpusEntry { text: "OneNote", activity: "记笔记" },
    ActivityCorpusEntry { text: "Typora", activity: "记笔记" },
    ActivityCorpusEntry { text: "Joplin", activity: "记笔记" },
    ActivityCorpusEntry { text: "印象笔记", activity: "记笔记" },
    ActivityCorpusEntry { text: "Markdown", activity: "记笔记" },
    ActivityCorpusEntry { text: "笔记", activity: "记笔记" },
    ActivityCorpusEntry { text: "便签", activity: "记笔记" },
    ActivityCorpusEntry { text: "待办", activity: "记笔记" },

    // ──────────────── 图片设计 ────────────────
    ActivityCorpusEntry { text: "Adobe Photoshop", activity: "图片设计" },
    ActivityCorpusEntry { text: "Figma", activity: "图片设计" },
    ActivityCorpusEntry { text: "Illustrator", activity: "图片设计" },
    ActivityCorpusEntry { text: "Canva", activity: "图片设计" },
    ActivityCorpusEntry { text: "图层", activity: "图片设计" },
    ActivityCorpusEntry { text: "设计稿", activity: "图片设计" },
    ActivityCorpusEntry { text: "UI 设计", activity: "图片设计" },
    ActivityCorpusEntry { text: "海报", activity: "图片设计" },
    ActivityCorpusEntry { text: "photoshop", activity: "图片设计" },

    // ──────────────── 视频剪辑 ────────────────
    ActivityCorpusEntry { text: "Adobe Premiere Pro", activity: "视频剪辑" },
    ActivityCorpusEntry { text: "剪映", activity: "视频剪辑" },
    ActivityCorpusEntry { text: "DaVinci Resolve", activity: "视频剪辑" },
    ActivityCorpusEntry { text: "Final Cut", activity: "视频剪辑" },
    ActivityCorpusEntry { text: "时间轴", activity: "视频剪辑" },
    ActivityCorpusEntry { text: "视频剪辑", activity: "视频剪辑" },
    ActivityCorpusEntry { text: "premiere", activity: "视频剪辑" },

    // ──────────────── 阅读 ────────────────
    ActivityCorpusEntry { text: "Kindle", activity: "阅读" },
    ActivityCorpusEntry { text: "PDF 阅读器", activity: "阅读" },
    ActivityCorpusEntry { text: "电子书", activity: "阅读" },
    ActivityCorpusEntry { text: "书籍", activity: "阅读" },
    ActivityCorpusEntry { text: "小说", activity: "阅读" },
    ActivityCorpusEntry { text: "阅读", activity: "阅读" },
    ActivityCorpusEntry { text: "翻页", activity: "阅读" },
    ActivityCorpusEntry { text: "reader", activity: "阅读" },

    // ──────────────── 学习 ────────────────
    ActivityCorpusEntry { text: "网课", activity: "学习" },
    ActivityCorpusEntry { text: "慕课", activity: "学习" },
    ActivityCorpusEntry { text: "课程", activity: "学习" },
    ActivityCorpusEntry { text: "课件", activity: "学习" },
    ActivityCorpusEntry { text: "Coursera", activity: "学习" },
    ActivityCorpusEntry { text: "edX", activity: "学习" },
    ActivityCorpusEntry { text: "背单词", activity: "学习" },
    ActivityCorpusEntry { text: "Anki", activity: "学习" },
    ActivityCorpusEntry { text: "学习", activity: "学习" },
    ActivityCorpusEntry { text: "刷题", activity: "学习" },

    // ──────────────── 文件管理 ────────────────
    ActivityCorpusEntry { text: "文件资源管理器", activity: "文件管理" },
    ActivityCorpusEntry { text: "File Explorer", activity: "文件管理" },
    ActivityCorpusEntry { text: "此电脑", activity: "文件管理" },
    ActivityCorpusEntry { text: "This PC", activity: "文件管理" },
    ActivityCorpusEntry { text: "文件夹", activity: "文件管理" },
    ActivityCorpusEntry { text: "Downloads", activity: "文件管理" },
    ActivityCorpusEntry { text: "回收站", activity: "文件管理" },
    ActivityCorpusEntry { text: "目录", activity: "文件管理" },

    // ──────────────── 系统设置 ────────────────
    ActivityCorpusEntry { text: "设置", activity: "系统设置" },
    ActivityCorpusEntry { text: "控制面板", activity: "系统设置" },
    ActivityCorpusEntry { text: "Settings", activity: "系统设置" },
    ActivityCorpusEntry { text: "Control Panel", activity: "系统设置" },
    ActivityCorpusEntry { text: "任务管理器", activity: "系统设置" },
    ActivityCorpusEntry { text: "Task Manager", activity: "系统设置" },
    ActivityCorpusEntry { text: "系统属性", activity: "系统设置" },
    ActivityCorpusEntry { text: "Device Manager", activity: "系统设置" },
    ActivityCorpusEntry { text: "设备管理器", activity: "系统设置" },
];