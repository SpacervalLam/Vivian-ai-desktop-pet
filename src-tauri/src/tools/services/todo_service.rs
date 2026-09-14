//! 待办服务 - 桥接待办列表与待办工具
//!
//! 提供待办列表的加载/保存共享访问器。

use crate::tools::builtin::todo_tools;

/// 待办服务：提供待办列表的初始化与持久化入口
pub struct TodoService;

impl TodoService {
    /// 从磁盘加载待办列表（应用启动时调用）
    pub fn load() {
        todo_tools::load_todo_list();
    }
}
