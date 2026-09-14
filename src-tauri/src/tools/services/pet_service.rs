//! 宠物服务 - 桥接桌宠动作队列与前端引擎
//!
//! 提供 pet_tools 动作队列的统一消费入口，前端引擎通过本服务取出待执行动作。

use crate::tools::builtin::pet_tools::{self, PetActionRequest};

/// 宠物服务：提供桌宠动作队列的消费入口
pub struct PetService;

impl PetService {
    /// 取出所有待处理的桌宠动作请求（可选按角色过滤）
    pub fn drain_pending_actions(character_id: Option<&str>) -> Vec<PetActionRequest> {
        pet_tools::drain_pending_actions(character_id)
    }

    /// 推送一个动作请求到队列
    pub fn push_action(req: PetActionRequest) {
        pet_tools::push_action(req);
    }
}
