/*
 * 手机端「过程」时间线的纯数据层。
 *
 * 与桌面工作页的 workActivitySummary 保持同一套口径：
 * - 工具调用按 tool_call_id 合并，结果优先于调用意图；
 * - 文件数只统计 read_file / write_file / edit_file 的不重复路径；
 * - 连续空搜索合并成一行，阶段计数按被合并的调用数累加；
 * - 路径以会话工作目录为基准折成相对路径，避免手机窄屏被绝对路径占满。
 *
 * 这里只做数据整理，不碰 DOM；companion-ui.js 负责渲染。
 */
(() => {
  'use strict';

  const KIND_ORDER = ['edit', 'read', 'search', 'run', 'browse', 'plan', 'image', 'other'];
  const FILE_TOOLS = new Set(['read_file', 'write_file', 'edit_file']);
  const PATH_KEYS = ['path', 'file_path', 'file', 'target', 'directory', 'dir'];
  const QUERY_KEYS = ['pattern', 'query', 'glob', 'keyword', 'q'];
  const FILE_BODY_KEYS = new Set(['content', 'new_string', 'old_string', 'old_str', 'new_str', 'body', 'text', 'patch']);

  const KIND_LABEL = {
    edit: '编辑', read: '读取', search: '搜索', run: '执行',
    browse: '浏览', plan: '计划', image: '图片', other: '工具',
  };
  const TOOL_LABEL = {
    read_file: '读取文件', write_file: '创建文件', edit_file: '编辑文件',
    grep_search: '搜索文件', glob: '查找文件', list_dir: '查看目录',
    run_command: '执行命令', web_search: '搜索网页', read_content: '读取网页',
    work_todo_write: '更新计划', read_image: '查看图片', generate_image: '生成图片',
  };

  function activityKind(name) {
    const tool = String(name || '').toLowerCase();
    return /search|grep|glob|find/.test(tool) ? 'search'
      : /read_content|web_fetch|browse|browser/.test(tool) ? 'browse'
        : /read_file|list_dir|list_files/.test(tool) ? 'read'
          : /write_file|edit_file|patch/.test(tool) ? 'edit'
            : /command|terminal|shell/.test(tool) ? 'run'
              : /todo|plan/.test(tool) ? 'plan'
                : /image/.test(tool) ? 'image' : 'other';
  }

  function workPhaseOf(name) {
    const kind = activityKind(name);
    if (kind === 'edit') return 'edit';
    if (kind === 'run') return 'verify';
    if (kind === 'search' || kind === 'read' || kind === 'browse') return 'explore';
    return 'other';
  }

  function argsObject(value) {
    if (!value) return {};
    if (typeof value === 'object') return Array.isArray(value) ? {} : value;
    if (typeof value !== 'string') return {};
    try {
      const parsed = JSON.parse(value);
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {};
    } catch {
      return {};
    }
  }

  /**
   * 会话里的 tool_use 有两种形状：
   * - 真实工作智能体：tool_arguments 是整批调用数组，每项带 id/name/arguments；
   * - 兼容旧接口：tool_name + tool_arguments 是单个调用对象。
   */
  function toolCallsOf(message, index) {
    const raw = message?.tool_arguments;
    if (Array.isArray(raw)) {
      return raw
        .map((entry, order) => {
          if (!entry || typeof entry !== 'object') return null;
          const name = entry.name || entry.tool_name || '';
          if (!name) return null;
          return {
            id: entry.id || entry.tool_call_id || `call-${index}-${order}`,
            name,
            args: argsObject(entry.arguments ?? entry.args ?? entry.input),
          };
        })
        .filter(Boolean);
    }
    const name = message?.tool_name || '';
    return name ? [{ id: message.tool_call_id || `call-${index}`, name, args: argsObject(raw) }] : [];
  }

  function argumentPath(args) {
    for (const key of PATH_KEYS) {
      const value = args?.[key];
      if (typeof value === 'string' && value.trim()) return value.trim();
    }
    return '';
  }

  function normalizePath(path) {
    return String(path || '').replace(/\\/g, '/').replace(/\/+/g, '/').replace(/^\//, '').toLowerCase();
  }

  function relativePath(path, cwd) {
    const value = String(path || '').trim();
    if (!value || !cwd) return value;
    const normalizedPath = value.replace(/\\/g, '/');
    const normalizedCwd = String(cwd).replace(/\\/g, '/').replace(/\/+$/, '');
    if (!normalizedCwd) return value;
    return normalizedPath.toLowerCase().startsWith(`${normalizedCwd.toLowerCase()}/`)
      ? normalizedPath.slice(normalizedCwd.length + 1)
      : value;
  }

  function middleEllipsis(value, max = 46) {
    const text = String(value || '');
    if (text.length <= max) return text;
    const head = Math.max(8, Math.floor((max - 1) * 0.38));
    const tail = Math.max(10, max - head - 1);
    return `${text.slice(0, head)}…${text.slice(-tail)}`;
  }

  function oneLine(value) {
    return String(value || '').replace(/\s+/g, ' ').trim();
  }

  function shortValue(value, max = 60) {
    const text = oneLine(typeof value === 'string' ? value : JSON.stringify(value));
    return text.length > max ? `${text.slice(0, max - 1)}…` : text;
  }

  function displayPath(path, cwd) {
    return middleEllipsis(relativePath(path, cwd), 46);
  }

  function argSummary(call) {
    const args = call.args || {};
    const path = argumentPath(args);
    if (call.kind === 'search') {
      for (const key of QUERY_KEYS) {
        const value = args[key];
        if (typeof value === 'string' && value.trim()) return shortValue(value, 56);
      }
      return '';
    }
    if (call.kind === 'run') {
      const command = args.command ?? args.script ?? args.cmd;
      return typeof command === 'string' ? shortValue(command, 58) : '';
    }
    if (call.kind === 'browse') {
      const url = args.url ?? args.query;
      return typeof url === 'string' ? shortValue(url, 56) : '';
    }
    if (path) return displayPath(path, call.cwd);
    for (const [key, value] of Object.entries(args)) {
      if (value === null || value === undefined) continue;
      if (typeof value === 'string' && value.trim()) return `${key}: ${shortValue(value, 46)}`;
      if (typeof value === 'number' || typeof value === 'boolean') return `${key}: ${value}`;
    }
    return '';
  }

  function parseJson(value) {
    if (typeof value !== 'string') return value && typeof value === 'object' ? value : null;
    const trimmed = value.trim();
    if (!trimmed || trimmed[0] !== '{' && trimmed[0] !== '[') return null;
    try { return JSON.parse(trimmed); } catch { return null; }
  }

  function unwrapData(value) {
    let data = value;
    for (let depth = 0; depth < 3; depth += 1) {
      if (!data || typeof data !== 'object') break;
      if (data.data && typeof data.data === 'object') { data = data.data; continue; }
      if (data.result && typeof data.result === 'object') { data = data.result; continue; }
      break;
    }
    return data;
  }

  function uniquePaths(values) {
    const seen = new Set(), result = [];
    for (const value of values || []) {
      if (typeof value !== 'string' || !value.trim()) continue;
      const key = normalizePath(value);
      if (seen.has(key)) continue;
      seen.add(key);
      result.push(value.trim());
    }
    return result;
  }

  function pathLikeLines(raw) {
    return String(raw || '').split(/\r?\n/)
      .map(line => line.trim())
      .filter(line => line.length > 2 && !line.startsWith('{') && !line.startsWith('[') && !/^(no|未|没有)/i.test(line))
      .slice(0, 120);
  }

  function describeResult(call, cwd) {
    if (!call.hasResult) {
      return { state: call.stopped ? 'stopped' : 'pending', meta: call.stopped ? '已停止' : '等待结果…', files: [], filesTotal: 0, path: call.path };
    }
    if (call.success === false) {
      return { state: 'failed', meta: '失败', files: [], filesTotal: 0, path: call.path };
    }
    const raw = call.result || '';
    const parsed = unwrapData(parseJson(raw));
    const info = { state: 'ok', meta: '已完成', files: [], filesTotal: 0, path: call.path || '' };

    if (call.kind === 'search') {
      if (parsed && Array.isArray(parsed.matches)) {
        const files = uniquePaths(parsed.matches.map(match => match?.file));
        const total = Number.isFinite(Number(parsed.total ?? parsed.count)) ? Number(parsed.total ?? parsed.count) : parsed.matches.length;
        info.files = files;
        info.filesTotal = files.length;
        info.meta = total > 0 ? `找到 ${total} 处匹配` : '未找到匹配';
        info.empty = total === 0;
        return info;
      }
      if (parsed && Array.isArray(parsed.files)) {
        const files = uniquePaths(parsed.files);
        info.files = files;
        info.filesTotal = Number.isFinite(Number(parsed.count)) ? Number(parsed.count) : files.length;
        info.meta = info.filesTotal > 0 ? `找到 ${info.filesTotal} 个文件` : '未找到文件';
        info.empty = info.filesTotal === 0;
        return info;
      }
      if (parsed && parsed.count != null && Number.isFinite(Number(parsed.count))) {
        const total = Number(parsed.count);
        info.filesTotal = total;
        info.meta = total > 0 ? `找到 ${total} 个文件` : '未找到文件';
        info.empty = total === 0;
        return info;
      }
      const lines = pathLikeLines(raw);
      if (lines.length) {
        info.files = uniquePaths(lines);
        info.filesTotal = info.files.length;
        info.meta = `找到 ${info.filesTotal} 个文件`;
        return info;
      }
      info.meta = '未找到匹配';
      info.empty = true;
      return info;
    }

    if (call.kind === 'read') {
      const lines = Number(parsed?.total_lines);
      const start = Number(call.args?.start_line ?? call.args?.offset);
      const end = Number(call.args?.end_line);
      if (Number.isFinite(lines) && lines > 0) info.meta = `${lines} 行`;
      else if (Number.isFinite(start) && Number.isFinite(end) && end >= start) info.meta = `第 ${start}-${end} 行`;
      else info.meta = '已读取';
      return info;
    }

    if (call.kind === 'edit') {
      const replacements = Number(parsed?.replacements);
      if (Number.isFinite(replacements) && replacements > 0) info.meta = `改动 ${replacements} 处`;
      else if (parsed?.total_lines) info.meta = `写入 ${Number(parsed.total_lines)} 行`;
      else info.meta = '已写入';
      return info;
    }

    if (call.kind === 'run') {
      const code = parsed?.exit_code != null ? Number(parsed.exit_code) : NaN;
      if (Number.isFinite(code)) {
        info.meta = code === 0 ? '已完成' : `退出码 ${code}`;
        if (code !== 0) info.state = 'failed';
      } else {
        const first = oneLine(raw).split(/\n/)[0];
        if (first && first.length <= 48) info.meta = first;
      }
      return info;
    }

    if (call.kind === 'browse') { info.meta = '已读取网页'; return info; }
    if (call.kind === 'plan') { info.meta = '计划已更新'; return info; }
    return info;
  }

  /**
   * 把会话消息整理成过程时间线。
   * @param {Array} messages 远程接口返回的消息（role: user/assistant/commentary/thinking/tool_use/tool_result/tool/...）
   * @param {string} cwd 会话工作目录（用于折相对路径）
   * @param {string} status 会话状态（running/idle/canceled）
   */
  function build(messages, cwd, status) {
    const source = Array.isArray(messages) ? messages : [];
    const sessionRunning = status === 'running';
    const calls = [];
    const byId = new Map();
    const byName = new Map();
    const items = [];

    const register = (call) => {
      calls.push(call);
      if (call.id) byId.set(call.id, call);
      const pending = byName.get(call.name) || [];
      byName.set(call.name, pending.filter(entry => entry.hasResult === false).concat(call));
    };
    const findPending = (id, name) => {
      if (id && byId.has(id)) {
        const call = byId.get(id);
        return call.hasResult ? null : call;
      }
      const pending = byName.get(name) || [];
      for (let index = pending.length - 1; index >= 0; index -= 1) {
        if (pending[index].hasResult === false) return pending[index];
      }
      return null;
    };

    source.forEach((message, index) => {
      const role = message?.role;
      if (role === 'tool_use' || role === 'tool' || role === 'tool_result') {
        if (role === 'tool_use') {
          const batch = toolCallsOf(message, index);
          batch.forEach((entry, order) => {
            const call = {
              id: entry.id,
              name: entry.name,
              index: index * 100 + order,
              args: entry.args,
              result: null,
              hasResult: false,
              success: null,
              cwd,
              kind: activityKind(entry.name),
              phase: workPhaseOf(entry.name),
              path: '',
            };
            call.path = relativePath(argumentPath(call.args), cwd);
            call.argsText = argSummary(call);
            call.info = describeResult(call, cwd);
            register(call);
            items.push({ type: 'tool', call, index: call.index, weight: 1 });
          });
          return;
        }
        const name = message.tool_name || '';
        if (!name) return;
        if (role === 'tool_result') {
          const target = findPending(message.tool_call_id, name);
          if (target) {
            target.result = message.content || '';
            target.hasResult = true;
            target.success = message.tool_success ?? null;
            target.info = describeResult(target, cwd);
            return;
          }
          const call = {
            id: message.tool_call_id || `result-${index}`,
            name,
            index: index * 100,
            args: argsObject(message.tool_arguments),
            result: message.content || '',
            hasResult: true,
            success: message.tool_success ?? null,
            cwd,
            kind: activityKind(name),
            phase: workPhaseOf(name),
            path: '',
          };
          call.path = relativePath(argumentPath(call.args), cwd);
          call.argsText = argSummary(call);
          call.info = describeResult(call, cwd);
          register(call);
          items.push({ type: 'tool', call, index, weight: 1 });
          return;
        }
        // 旧版接口把调用与结果压成一条 role: 'tool' 的消息。
        const call = {
          id: message.tool_call_id || `tool-${index}`,
          name,
          index,
          args: argsObject(message.tool_arguments),
          result: message.content || '',
          hasResult: true,
          success: message.tool_success ?? null,
          cwd,
          kind: activityKind(name),
          phase: workPhaseOf(name),
          path: '',
        };
        call.path = relativePath(argumentPath(call.args), cwd);
        call.argsText = argSummary(call);
        call.info = describeResult(call, cwd);
        register(call);
        items.push({ type: 'tool', call, index, weight: 1 });
        return;
      }
      if (role === 'thinking' && message.content) {
        items.push({ type: 'think', content: message.content, index });
        return;
      }
      if (role === 'commentary' && message.content) {
        items.push({ type: 'say', content: message.content, index });
      }
    });

    // 合并相邻的「空搜索结果」，并记录被合并的调用数供阶段计数使用。
    const merged = [];
    for (const item of items) {
      if (item.type !== 'tool') {
        merged.push(item);
        continue;
      }
      const info = item.call.info || describeResult(item.call, cwd);
      const previous = merged[merged.length - 1];
      const previousInfo = previous?.type === 'tool'
        ? previous.call.info || describeResult(previous.call, cwd)
        : null;
      if (info.empty && previousInfo?.empty && previous.type === 'tool') {
        previous.weight += item.weight;
        previous.emptyCount = (previous.emptyCount || 1) + 1;
        continue;
      }
      item.emptyCount = info.empty ? 1 : undefined;
      merged.push(item);
    }

    // 阶段只在工具调用达到一定密度时出现，避免标题比内容还多。
    const toolItems = merged.filter(item => item.type === 'tool');
    const totalSteps = toolItems.reduce((sum, item) => sum + item.weight, 0);
    if (totalSteps >= 4) {
      let current = null;
      for (const item of toolItems) {
        const phase = item.call.phase;
        if (!current || current.phase !== phase) {
          current = { phase, count: 0, files: new Set(), failed: 0 };
          item.phaseStart = current;
        }
        current.count += item.weight;
        if (FILE_TOOLS.has(item.call.name) && item.call.path) current.files.add(normalizePath(item.call.path));
        if (item.call.success === false || item.call.info?.state === 'failed') current.failed += 1;
      }
      for (const item of toolItems) {
        if (item.phaseStart) item.phaseStart.files = item.phaseStart.files.size;
      }
    }

    // 序号在前的先出现；被合并的空结果沿用首条调用的位置。
    const ordered = [];
    let currentPhase = null;
    for (const item of merged) {
      if (item.type === 'tool' && item.phaseStart) {
        const info = item.phaseStart;
        ordered.push({ type: 'phase', phase: info.phase, count: info.count, files: info.files, failed: info.failed, index: item.index });
        currentPhase = info.phase;
      }
      ordered.push(item);
    }
    // 没有任何工具行却留下阶段头时移除（防御式，正常不会发生）。
    const timeline = ordered.filter(item => item.type !== 'phase' || item.count > 0);

    const facts = [];
    let failed = 0;
    const counts = new Map();
    for (const call of calls) {
      const info = describeResult(call, cwd);
      call.info = info;
      const entry = counts.get(call.kind) || { count: 0, files: new Set() };
      entry.count += 1;
      if (FILE_TOOLS.has(call.name) && call.path) entry.files.add(normalizePath(call.path));
      counts.set(call.kind, entry);
      if (call.success === false || info.state === 'failed') failed += 1;
    }
    for (const kind of KIND_ORDER) {
      const entry = counts.get(kind);
      if (entry) facts.push({ kind, count: entry.count, files: entry.files.size });
    }

    const thinking = merged.filter(item => item.type === 'think').length;
    const fileCalls = calls.filter(call => FILE_TOOLS.has(call.name)).length;
    let emptyTotal = 0, emptyMerged = 0;
    for (const item of merged) {
      if (item.type === 'tool' && item.emptyCount) {
        emptyTotal += item.emptyCount;
        emptyMerged += 1;
      }
    }

    return {
      cwd,
      calls,
      items: timeline,
      summary: { facts, total: calls.length, failed },
      counts: { failed, files: fileCalls, thinking },
      steps: totalSteps,
      running: sessionRunning,
      emptyTotal,
      emptyMerged,
      currentPhase,
    };
  }

  function summaryText(summary) {
    const facts = summary?.facts || [];
    return facts.map(fact => {
      if ((fact.kind === 'edit' || fact.kind === 'read') && fact.files > 0) {
        return `${KIND_LABEL[fact.kind]} ${fact.files} 个文件`;
      }
      if (fact.kind === 'edit') return `编辑 ${fact.count} 次`;
      if (fact.kind === 'read') return `读取 ${fact.count} 次`;
      if (fact.kind === 'search') return `搜索 ${fact.count} 次`;
      if (fact.kind === 'run') return `执行 ${fact.count} 条命令`;
      if (fact.kind === 'browse') return `读取 ${fact.count} 个网页`;
      if (fact.kind === 'plan') return `更新计划 ${fact.count} 次`;
      if (fact.kind === 'image') return `处理 ${fact.count} 张图片`;
      return `执行工具 ${fact.count} 次`;
    }).join(' · ');
  }

  function toolLabel(name) {
    return TOOL_LABEL[name] || name || '工具';
  }

  function phaseLabel(phase) {
    return { explore: '探索', edit: '编辑', verify: '验证', other: '其他' }[phase] || '其他';
  }

  function detailPairs(call) {
    const pairs = [];
    for (const [key, value] of Object.entries(call.args || {})) {
      if (FILE_BODY_KEYS.has(key)) {
        pairs.push([key, `（${String(value).length} 字符）`]);
        continue;
      }
      if (value === null || value === undefined || value === '') continue;
      if (typeof value === 'object') pairs.push([key, JSON.stringify(value)]);
      else pairs.push([key, String(value)]);
    }
    return pairs;
  }

  window.WorkProcess = {
    build,
    summaryText,
    toolLabel,
    phaseLabel,
    activityKind,
    workPhaseOf,
    relativePath,
    displayPath,
    middleEllipsis,
    argSummary,
    describeResult,
    detailPairs,
    FILE_TOOLS,
    KIND_LABEL,
  };
})();
