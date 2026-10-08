"use strict";
var MemoryPresentation = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // src/mobileMemory.ts
  var mobileMemory_exports = {};
  __export(mobileMemory_exports, {
    conversationThreads: () => conversationThreads,
    isMemorySummary: () => isMemorySummary,
    isVisibleMemoryFact: () => isVisibleMemoryFact,
    sharedEventTimeline: () => sharedEventTimeline,
    splitActionText: () => splitActionText
  });

  // src/components/mind-inspector/pages/memoryPresentation.ts
  var speakerName = (value, character) => {
    const key = value.trim().toLowerCase();
    if (key === "i" || key === "me") return character === "vivian" ? "Vivian" : "Nana";
    if (key === "user") return "\u7528\u6237";
    if (key === "vivian") return "Vivian";
    if (key === "nana") return "Nana";
    return value.trim();
  };
  var isMemoryFact = (item) => item.metadata?.record_kind === "fact";
  var isMemorySummary = (item) => item.metadata?.record_kind === "session_summary";
  var isVisibleMemoryFact = (item) => !["system_seed", "environment_preset"].includes(String(item.metadata?.source ?? "")) && !!item.content.trim() && !item.consolidated && !["subjective", "observation", "internal"].includes(String(item.metadata?.record_kind ?? "")) && isMemoryFact(item);
  var sharedEventTimeline = (items, records) => {
    const originals = new Map(records.flatMap((record) => record.turns.map((turn) => [turn.id, { turn, conversationId: record.id }])));
    const groups = /* @__PURE__ */ new Map();
    for (const item of items) {
      if (!isMemorySummary(item) || item.consolidated || item.metadata?.index_active === false || item.metadata?.event_schema_version !== 1) continue;
      const parts = item.metadata?.summary_parts;
      if (!Array.isArray(parts)) continue;
      for (const part of parts) {
        if (!Array.isArray(part?.events)) continue;
        for (const event of part.events) {
          if (!event || typeof event.id !== "string" || typeof event.title !== "string" || typeof event.detail !== "string" || typeof event.source_quote !== "string" || !event.source_quote.trim() || !["planned", "started", "progressed", "completed", "cancelled"].includes(event.phase)) continue;
          const source = originals.get(event.source_message_id);
          if (!source || source.turn.speaker !== "user" || !source.turn.text.includes(event.source_quote)) continue;
          let group = groups.get(event.id);
          if (!group) {
            group = { id: event.id, title: event.title, progress: [], conversationIds: [], time: 0, searchText: "" };
            groups.set(event.id, group);
          }
          if (group.progress.some((p) => p.source_message_id === event.source_message_id)) continue;
          group.progress.push({ ...event, recorded_at: source.turn.timestamp, conversationId: source.conversationId });
        }
      }
    }
    for (const group of groups.values()) {
      group.progress.sort((a, b) => a.recorded_at - b.recorded_at || a.source_message_id.localeCompare(b.source_message_id));
      group.conversationIds = [...new Set(group.progress.map((p) => p.conversationId))];
      group.time = group.progress[group.progress.length - 1]?.recorded_at ?? 0;
      group.searchText = `${group.title} ${group.progress.map((p) => `${p.detail} ${p.source_quote}`).join(" ")}`;
    }
    return [...groups.values()].sort((a, b) => b.time - a.time || a.id.localeCompare(b.id));
  };
  var conversationThreads = (records, character) => records.map((record) => ({
    id: record.id,
    title: record.title,
    time: record.ended_at,
    startedAt: record.started_at,
    canonical: true,
    turns: record.turns.map((turn) => ({
      id: turn.id,
      speaker: speakerName(turn.speaker, character),
      audience: speakerName(turn.listener === "all" ? "\u5927\u5BB6" : turn.listener, character),
      text: turn.text,
      timestamp: turn.timestamp,
      sticker: turn.sticker
    })),
    items: [],
    searchText: `${record.title} ${record.turns.map((turn) => turn.text).join(" ")}`
  })).sort((a, b) => b.time - a.time);

  // src/utils/ActionText.ts
  function splitActionText(text) {
    const parts = [];
    let start = 0;
    let depth = 0;
    for (let i = 0; i < text.length; i++) {
      if (text[i] === "\uFF08" || text[i] === "(") {
        if (depth === 0) {
          if (i > start) parts.push({ text: text.slice(start, i), action: false });
          start = i;
        }
        depth++;
      } else if ((text[i] === "\uFF09" || text[i] === ")") && depth > 0) {
        depth--;
        if (depth === 0) {
          parts.push({ text: text.slice(start, i + 1), action: true });
          start = i + 1;
        }
      }
    }
    if (start < text.length) parts.push({ text: text.slice(start), action: depth > 0 });
    return parts;
  }
  return __toCommonJS(mobileMemory_exports);
})();
