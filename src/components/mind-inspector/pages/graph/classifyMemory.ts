/**
 * 记忆 / 日记 → 图谱节点分类
 *
 * 把原始记忆条目分类为具体的节点类型（对话 / 内心OS / 旁观记录 / 总结 / 重要事件等），
 * 并给出该节点应连接的核心节点角色（user / agent / roommate）。
 */

import { COLORS, CHARACTER_ACCENT } from '../../design-system';
import type { MemoryItem } from '../../../../types';
import type { CharacterId, GraphNode, NodeType } from './types';

export const parseTimestamp = (val: string | number): number => {
  if (typeof val === 'number') return val * 1000;
  const t = Date.parse(val);
  return isNaN(t) ? Date.now() : t;
};

export interface ClassifyContext {
  character: CharacterId;
  roommateChar: CharacterId;
  now: number;
}

/** 节点需要连接的核心节点角色，由调用方映射为实际节点 id */
export type EdgeRole = 'user' | 'agent' | 'roommate';

export interface MemoryNodeResult {
  node: GraphNode;
  edgeRoles: EdgeRole[];
}

/** 日记节点所需的轻量字段 */
export interface DiaryLite {
  id: string;
  date: string;
  content: string;
  created_at: number;
}

/**
 * 将一条记忆分类为图谱节点。
 *
 * 日记索引记忆（tags 含 diary 或 metadata.kind=diary）返回 null，其内容由日记节点承载。
 */
export function memoryToGraphNode(m: MemoryItem, ctx: ClassifyContext): MemoryNodeResult | null {
  const { character, roommateChar } = ctx;
  const id = `episode:${m.id}`;
  const tagSet = new Set(m.tags);

  // 日记索引记忆（tags 含 diary 或 metadata.kind=diary）不作为独立节点渲染，其内容由日记节点承载
  if (tagSet.has('diary') || m.metadata?.kind === 'diary') return null;

  let content = m.content || '';
  const memType = m.memory_type || '';

  // 解析说话者前缀（支持所有格式）：
  // - "[User says to me] xxx" / "[Vivian says to me] xxx" → 入站（第一人称接收）
  // - "[I say to User] xxx" / "[I say to Vivian] xxx" → 出站（第一人称发送）
  // - "[User says to Vivian] xxx" / "[Vivian says to User] xxx" → 旁观（第三人称）
  // - 旧数据兼容: "[X says to you] xxx"
  // 返回 { speaker, listener, content }，speaker/listener 归一化为小写 ID
  const parseSpeakerPrefix = (text: string): { speaker: string; listener: string; content: string } | null => {
    if (!text.startsWith('[')) return null;
    const closeBracket = text.indexOf(']');
    if (closeBracket === -1) return null;
    const inside = text.slice(1, closeBracket);
    const parts = inside.split(/\s+/);
    // 格式: Speaker say/says to Listener
    if (parts.length !== 4 || parts[2] !== 'to' || (parts[1] !== 'say' && parts[1] !== 'says')) return null;
    const speakerName = parts[0];
    const listenerName = parts[3];
    const rest = text.slice(closeBracket + 1).trimStart();
    const normalize = (name: string): string => {
      switch (name) {
        case 'I': return '__self__';
        case 'me': return '__self__';
        case 'you': return '__self__';
        case 'User': return 'user';
        case 'Vivian': return 'vivian';
        case 'Nana': return 'nana';
        default: return name.toLowerCase();
      }
    };
    return {
      speaker: normalize(speakerName),
      listener: normalize(listenerName),
      content: rest,
    };
  };

  let prefixInfo = parseSpeakerPrefix(content);
  let contentSpeaker = '';
  let contentListener = '';
  if (prefixInfo) {
    // 将 __self__ 解析为当前角色 ID
    if (prefixInfo.speaker === '__self__') {
      contentSpeaker = character;
    } else {
      contentSpeaker = prefixInfo.speaker;
    }
    if (prefixInfo.listener === '__self__') {
      contentListener = character;
    } else {
      contentListener = prefixInfo.listener;
    }
    content = prefixInfo.content;
  }

  // 内心OS：类型标记 + tag + 内容前缀检测
  const hasInnerOsPrefix = content.startsWith('内心OS') || content.startsWith('内心OS：') || content.startsWith('内心OS:') || content.startsWith('（内心OS') || content.startsWith('(内心OS');
  const isInnerThought =
    memType === 'inner_monologue' ||
    tagSet.has('inner_os') ||
    tagSet.has('inner_monologue') ||
    hasInnerOsPrefix;

  if (isInnerThought) {
    // 第一步：去掉开头的 "内心OS：" / "内心OS:" 前缀
    content = content.replace(/^内心OS[：:]\s*/, '');
    // 第二步：去掉开头的括号包裹形式 "（内心OS：..." / "(内心OS：..."
    content = content.replace(/^[（(]\s*内心OS[：:]?\s*/, '');
    // 第三步：去掉首尾成对的括号（全角/半角）
    // 先去掉结尾的括号，再检查开头是否有孤立的左括号，一并去掉
    content = content.replace(/\s*[）)]\s*$/, '');
    content = content.replace(/^[（(]\s*/, '');
    content = content.trim();

    if (content.startsWith('{')) {
      let extracted: string | undefined;
      try {
        const parsed = JSON.parse(content);
        const val = parsed.monologue || parsed.text || parsed.content || parsed.thought || parsed.output;
        if (typeof val === 'string' && val.trim()) {
          extracted = val.trim();
        }
      } catch {
        // JSON 畸形/截断 → 正则兜底提取（处理转义引号）
        const re = /"(?:monologue|text|content|thought|output)"\s*:\s*"((?:[^"\\]|\\[\s\S])*)"/i;
        const match = content.match(re);
        if (match && match[1]) {
          extracted = match[1]
            .replace(/\\n/g, '\n')
            .replace(/\\"/g, '"')
            .replace(/\\\\/g, '\\')
            .trim();
        }
      }
      if (extracted) {
        content = extracted;
      }
    }
  }
  const isImportantEvent = memType === 'important_event';

  // 优先使用后端明确标记的 content_kind 标签
  const hasDialogueTurnTag = tagSet.has('dialogue_turn');
  // 话题总结标签（兼容旧版 user_dialogue_summary / agent_dialogue_summary）
  const hasTopicSummaryTag = tagSet.has('topic_summary')
    || tagSet.has('user_dialogue_summary')
    || tagSet.has('agent_dialogue_summary');
  const hasExtractedTag = tagSet.has('extracted_memory');

  // 类型判断：memory_type 现在正确序列化，casual_conversation 都是直接对话原文
  const isCasualType = memType === 'casual_conversation';
  const isShortTermType = memType === 'short_term';
  const isEmptyMemType = !memType || memType.trim() === '';

  // AI 发言的标志 tags（proactive/greeting/chat_augment 都是 AI 主动说的话）
  const hasUserTag = tagSet.has('user');
  const hasAssistantTag = tagSet.has('assistant');
  const hasGreetingTag = tagSet.has('startup_greeting') || tagSet.has('wake_greeting');
  const hasProactiveTag = tagSet.has('proactive');
  const hasChatAugmentTag = tagSet.has('chat_augment');
  const hasDialogueTag = tagSet.has('dialogue');
  const hasPresenceLogTag = tagSet.has('presence_log');

  // 在场状态切换事件（presence_log）不渲染为图谱节点，仅通过时间轴颜色反映
  if (hasPresenceLogTag) return null;

  // 工具执行记录（tool_call）不渲染为图谱节点
  if (tagSet.has('tool_call')) return null;

  // 跨角色对话总结（tags 同时含 cross_character + topic_summary）不渲染：
  // 这类记忆是后端对本次跨角色对话的机械拼接总结（"我和X聊了聊：我对她说…她回复我…"），
  // 逐条对话原文已作为 dialogue 节点展示，拼接总结对用户无额外信息价值。
  if (hasTopicSummaryTag && tagSet.has('cross_character')) return null;

  const isAISpeech = hasAssistantTag && (hasGreetingTag || hasProactiveTag || hasChatAugmentTag || hasDialogueTag);

  // 旁观对话判定：metadata.perspective === 'observer' 或 tags 含 bystander/overheard
  // 这类记忆是对话原文（CasualConversation），由旁观者存储，前端用半透明节点表示旁观
  const metaPerspective = (m.metadata && typeof m.metadata === 'object')
    ? (m.metadata as Record<string, unknown>).perspective
    : undefined;
  const isBystanderDialogue = (typeof metaPerspective === 'string' && metaPerspective === 'observer')
    || tagSet.has('bystander');

  // ShortTerm 缓冲中的对话轮次（用户消息/AI回复）：
  // - 有 dialogue_turn 标签 → 新数据直接识别
  // - 旧数据兼容：(short_term 类型 OR memory_type 为空) + (user/assistant 标签) + 非 presence_log
  const isShortTermTurn = (isShortTermType || isEmptyMemType) && (hasUserTag || hasAssistantTag) && !hasPresenceLogTag
    && !isInnerThought && !hasTopicSummaryTag && !isImportantEvent;

  // 用户发言判定（兼容旧数据）：有 user 标签且不是其他特殊类型
  const isUserSpeech = hasUserTag && !hasExtractedTag && !hasPresenceLogTag
    && !isInnerThought && !hasTopicSummaryTag && !isImportantEvent
    && !tagSet.has('cross_character') && !tagSet.has(roommateChar);

  // 直接对话原文判定：
  // 1. 明确的 dialogue_turn 标签
  // 2. casual_conversation 类型（且非总结标签）
  // 3. ShortTerm 缓冲中的对话轮次（含 memory_type 为空的旧数据兼容）
  // 4. AI 发言标志 tags 组合（兼容旧数据）
  // 5. 用户发言标志（兼容旧数据：有 user 标签且非其他特殊类型）
  // 6. 旁观对话（带 perspective=observer 标记的 CasualConversation）
  const isDirectDialogue = hasDialogueTurnTag
    || (isCasualType && !hasTopicSummaryTag)
    || isShortTermTurn
    || (isAISpeech && !hasTopicSummaryTag)
    || isUserSpeech
    || isBystanderDialogue;
  const isCrossCharDialogue = isDirectDialogue && (tagSet.has('cross_character') || tagSet.has(roommateChar) || (contentSpeaker !== '' && contentSpeaker !== 'user')) && !hasTopicSummaryTag;

  // 话题总结判定（合并原用户对话总结 + 角色对话总结）：
  // - topic_summary 标签（新数据）
  // - user_dialogue_summary / agent_dialogue_summary 标签（旧数据兼容）
  // - 旧数据兼容：extracted_memory + user tag = 用户相关总结；extracted_memory + cross_character/roommate tag = 跨角色总结
  const isTopicSummary = hasTopicSummaryTag
    || (hasExtractedTag && !isDirectDialogue && !isInnerThought && !isImportantEvent
      && (tagSet.has('user') || tagSet.has(roommateChar) || tagSet.has('cross_character')));

  const ts = parseTimestamp(m.timestamp ?? m.created_at);

  // 发言者识别：content 前缀优先于 metadata.speaker，再回退到 tag 判定
  // metadata.speaker 取值：'user' / 角色 ID
  const metaSpeaker = (m.metadata && typeof m.metadata === 'object')
    ? (m.metadata as Record<string, unknown>).speaker
    : undefined;
  const metaSpeakerStr = typeof metaSpeaker === 'string' ? metaSpeaker : '';
  const metaListener = (m.metadata && typeof m.metadata === 'object')
    ? (m.metadata as Record<string, unknown>).listener
    : undefined;
  const metaListenerStr = typeof metaListener === 'string' ? metaListener : '';
  // content 前缀是更可靠的证据
  const effectiveSpeaker = contentSpeaker || metaSpeakerStr;
  const effectiveListener = contentListener || metaListenerStr;
  // 用户发言：effectiveSpeaker === 'user' 或带 user tag（非跨角色）
  const isUserSpeaker = effectiveSpeaker === 'user' || (!effectiveSpeaker && hasUserTag && !isCrossCharDialogue);
  // 智能体发言：effectiveSpeaker 为角色 ID，或带 assistant tag
  const isAgentSpeaker = effectiveSpeaker === character
    || effectiveSpeaker === roommateChar
    || (!effectiveSpeaker && (hasAssistantTag || isAISpeech) && !isUserSpeaker);

  // 微信渠道检测（metadata.channel 或 tag），私聊和群聊均使用信封图标
  const isWechat = m.metadata?.channel === 'wechat' || m.metadata?.channel === 'wechat_group' || tagSet.has('wechat');

  // 阅读/知识文档检测（仅类型1：内化知识 → 存知识库）：
  // - 后台知识采集写入的网络源知识文档（kind=knowledge_document + source=web，tags含 knowledge/document）
  // - memory_type === 'knowledge' 的知识条目
  // 类型2（分享链接 → 微信面板）不在此判定内：它带 link_card/kind=web_link 字段 + channel=wechat，
  // 由下方 isWechat && isDirectDialogue 分支识别为信封节点。
  const metaKind = (m.metadata && typeof m.metadata === 'object')
    ? (m.metadata as Record<string, unknown>).kind
    : undefined;
  const metaSource = (m.metadata && typeof m.metadata === 'object')
    ? (m.metadata as Record<string, unknown>).source
    : undefined;
  const isWebKnowledgeDoc = metaKind === 'knowledge_document' && metaSource === 'web'
    && tagSet.has('knowledge') && tagSet.has('document');
  const isKnowledgeType = memType === 'knowledge';
  const isReading = isWebKnowledgeDoc || isKnowledgeType;

  let nodeType: NodeType = 'episode';
  let nodeColor: string = COLORS.event.observation;
  // 将参与者 ID 映射到边角色
  const participantToRole = (p: string): EdgeRole | null => {
    if (p === 'user') return 'user';
    if (p === character) return 'agent';
    if (p === roommateChar) return 'roommate';
    return null;
  };

  // 已摘要标记：metadata.summarized === true 的原始对话仍渲染为 dialogue 节点，
  // 但在 GraphPage 中会被折叠到对应的 session_summary 父节点下
  const isSummarized = m.metadata?.summarized === true;

  // SessionSummary 类型识别（含旧版 eviction_merge / stage0_compress 数据兼容）
  // 注意：显式排除 isReading（知识文档/链接分享），避免被误判为 session_summary
  const isSessionSummary = !isReading && (
    memType === 'session_summary'
    || tagSet.has('session_summary')
    || m.metadata?.consolidation_stage === 'eviction_merge'
    || m.metadata?.consolidation_stage === 'stage0_compress'
  );

  if (isSessionSummary) {
    nodeType = 'session_summary';
    nodeColor = '#7C3AED';
  } else if (isReading) {
    // 阅读/链接/知识文档：放在高优先级，确保链接分享和知识采集结果正确归类为 🔗 节点
    nodeType = 'reading';
    nodeColor = COLORS.event.reading;
  } else if (isInnerThought) {
    nodeType = 'inner_thought';
    nodeColor = COLORS.event.mood;
  } else if (isWechat && isDirectDialogue) {
    nodeType = 'wechat';
    if (isUserSpeaker) {
      nodeColor = COLORS.event.dialogue;
    } else if (effectiveSpeaker === roommateChar) {
      nodeColor = CHARACTER_ACCENT[roommateChar];
    } else if (isAgentSpeaker || effectiveSpeaker === character) {
      nodeColor = CHARACTER_ACCENT[character];
    } else {
      nodeColor = isCrossCharDialogue ? CHARACTER_ACCENT[roommateChar] : CHARACTER_ACCENT[character];
    }
  } else if (isDirectDialogue) {
    nodeType = 'dialogue';
    // 旁观对话：颜色使用说话者的主题色
    if (isBystanderDialogue) {
      if (isUserSpeaker) {
        nodeColor = COLORS.event.dialogue;
      } else if (effectiveSpeaker === character) {
        nodeColor = CHARACTER_ACCENT[character];
      } else if (effectiveSpeaker === roommateChar) {
        nodeColor = CHARACTER_ACCENT[roommateChar];
      } else {
        nodeColor = COLORS.event.dialogue;
      }
    } else if (isUserSpeaker) {
      nodeColor = COLORS.event.dialogue;
    } else if (effectiveSpeaker === roommateChar) {
      nodeColor = CHARACTER_ACCENT[roommateChar];
    } else if (isAgentSpeaker || effectiveSpeaker === character) {
      nodeColor = CHARACTER_ACCENT[character];
    } else {
      // 兜底：跨角色对话用室友色，否则用智能体色
      nodeColor = isCrossCharDialogue ? CHARACTER_ACCENT[roommateChar] : CHARACTER_ACCENT[character];
    }
  } else if (isTopicSummary) {
    nodeType = 'topic_summary';
    // 话题总结颜色：根据 subject 区分（user 相关用绿色，跨角色用粉色）
    nodeColor = (tagSet.has('user') && !tagSet.has('cross_character') && !tagSet.has(roommateChar))
      ? '#34C759' : '#ec4899';
  } else if (isImportantEvent) {
    nodeType = 'important_event';
    nodeColor = COLORS.danger;
  }

  // 对话节点的真实说话者（与颜色判定同款优先级），供回应箭头判定使用
  let speaker: string | undefined;
  if (nodeType === 'dialogue' || nodeType === 'wechat') {
    if (isBystanderDialogue) {
      speaker = effectiveSpeaker || undefined;
    } else if (isUserSpeaker) {
      speaker = 'user';
    } else if (effectiveSpeaker === roommateChar) {
      speaker = roommateChar;
    } else if (isAgentSpeaker || effectiveSpeaker === character) {
      speaker = character;
    } else {
      speaker = isCrossCharDialogue ? roommateChar : character;
    }
  }

  const node: GraphNode = {
    id,
    type: nodeType,
    label: content.length > 16 ? content.slice(0, 16) + '…' : content,
    color: nodeColor,
    importance: nodeType === 'topic_summary' ? 0.75 : m.importance,
    preview: content,
    timestamp: ts,
    side: 'left',
    speaker,
    sessionId: m.episode_id || (m.metadata && typeof m.metadata === 'object' ? (m.metadata as Record<string, unknown>).session_id as string | undefined : undefined) || undefined,
    memoryId: m.id,
    metadata: (m.metadata && typeof m.metadata === 'object') ? (m.metadata as Record<string, unknown>) : undefined,
    bystander: isBystanderDialogue && nodeType === 'dialogue' ? true : undefined,
    summarized: isSummarized ? true : undefined,
  };

  let edgeRoles: EdgeRole[];
  if (nodeType === 'dialogue' || nodeType === 'wechat') {
    if (isBystanderDialogue) {
      // 旁观对话：边连接到实际参与者（说话者 + 听者），而非观察者
      const roles = new Set<EdgeRole>();
      const spkRole = participantToRole(effectiveSpeaker);
      const lstRole = participantToRole(effectiveListener);
      if (spkRole) roles.add(spkRole);
      if (lstRole) roles.add(lstRole);
      edgeRoles = Array.from(roles);
      if (edgeRoles.length === 0) edgeRoles = ['user', 'agent'];
    } else {
      edgeRoles = isCrossCharDialogue ? ['agent', 'roommate'] : ['user', 'agent'];
    }
  } else if (nodeType === 'topic_summary') {
    // 话题总结：根据 subject 区分连接边
    edgeRoles = (tagSet.has('user') && !tagSet.has('cross_character') && !tagSet.has(roommateChar))
      ? ['user', 'agent'] : ['agent', 'roommate'];
  } else {
    edgeRoles = ['agent'];
  }

  return { node, edgeRoles };
}

/**
 * 将一条记忆分类为图谱节点。
 *
 * 所有内容都在单节点中展示，不再按换行拆分多节点。
 * label 取第一行用于图上简短显示，preview 保留完整内容供 tooltip 展示。
 */
export function memoryToGraphNodes(m: MemoryItem, ctx: ClassifyContext): MemoryNodeResult[] {
  const base = memoryToGraphNode(m, ctx);
  if (!base) return [];

  const content = base.node.preview || '';
  const firstLine = content.split('\n').find((l) => l.trim().length > 0) || content;
  const trimmed = firstLine.trim();
  if (trimmed && trimmed !== content) {
    base.node.label = trimmed.length > 16 ? trimmed.slice(0, 16) + '…' : trimmed;
  }

  return [base];
}

/** 将一条日记转换为图谱节点（固定连接到 agent 核心节点） */
export function diaryToGraphNode(d: DiaryLite, ctx: ClassifyContext): GraphNode {
  const ts = d.created_at ? (d.created_at < 1e12 ? d.created_at * 1000 : d.created_at) : ctx.now;
  const diaryColor = '#8B4513';
  return {
    id: `diary:${d.id}`,
    type: 'diary',
    label: d.date,
    color: diaryColor,
    importance: 0.75,
    preview: d.content.length > 80 ? d.content.slice(0, 80) + '…' : d.content,
    timestamp: ts,
    side: 'left',
  };
}
