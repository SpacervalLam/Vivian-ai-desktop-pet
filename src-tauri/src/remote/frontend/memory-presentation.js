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
    isVisibleOngoingTask: () => isVisibleOngoingTask,
    ongoingLabel: () => ongoingLabel,
    ongoingSearchText: () => ongoingSearchText,
    ongoingTaskOf: () => ongoingTaskOf,
    registerCharacters: () => registerCharacters,
    sharedEventTimeline: () => sharedEventTimeline,
    splitActionText: () => splitActionText
  });

  // src/characters.js
  var BUILTIN_CHARACTER_IDS = ["vivian", "nana"];
  var CHARACTER_IDS = [...BUILTIN_CHARACTER_IDS];
  var builtinNames = /* @__PURE__ */ new Map([["vivian", "Vivian"], ["nana", "Nana"]]);
  var names = new Map(builtinNames);
  function registerCharacters(packages) {
    names = new Map(builtinNames);
    for (const { id, name } of packages) {
      if (!names.has(id)) names.set(id, name);
    }
    CHARACTER_IDS.splice(0, CHARACTER_IDS.length, ...names.keys());
  }
  function isCharacterId(value) {
    return names.has(value);
  }
  function characterName(id) {
    return names.get(id) || id || "Vivian";
  }

  // src/i18n/records.js
  var rows = {
    thoughtLabel: ["\u6B63\u5728\u60F3", "On my mind", "\u4ECA\u8003\u3048\u3066\u3044\u308B\u3053\u3068"],
    moodLabel: ["\u5FC3\u60C5", "Mood", "\u6C17\u5206"],
    emotionsLabel: ["\u60C5\u7EEA", "Emotions", "\u611F\u60C5"],
    needsLabel: ["\u9700\u6C42", "Needs", "\u6B32\u6C42"],
    relationsLabel: ["\u5173\u7CFB", "Relationships", "\u95A2\u4FC2"],
    noteFullscreen: ["\u5168\u5C4F\u9605\u8BFB", "Read fullscreen", "\u5168\u753B\u9762\u3067\u8AAD\u3080"],
    noteExitFullscreen: ["\u9000\u51FA\u5168\u5C4F", "Exit fullscreen", "\u5168\u753B\u9762\u3092\u9589\u3058\u308B"],
    conversationTitle: ["\u4E0E{{names}}\u7684\u5BF9\u8BDD", "Conversation with {{names}}", "{{names}}\u3068\u306E\u4F1A\u8A71"],
    conversationParticipants: ["{{first}}\u4E0E{{others}}\u7684\u5BF9\u8BDD", "Conversation between {{first}} and {{others}}", "{{first}}\u3068{{others}}\u306E\u4F1A\u8A71"],
    conversationYou: ["\u4F60", "you", "\u3042\u306A\u305F"],
    user: ["\u7528\u6237", "You", "\u30E6\u30FC\u30B6\u30FC"],
    everyone: ["\u5927\u5BB6", "Everyone", "\u307F\u3093\u306A"],
    pageFailed: ["\u8FD9\u4E00\u9875\u52A0\u8F7D\u5931\u8D25\u4E86", "This page failed to load", "\u3053\u306E\u30DA\u30FC\u30B8\u3092\u8AAD\u307F\u8FBC\u3081\u307E\u305B\u3093\u3067\u3057\u305F"],
    pageRetry: ["\u91CD\u8BD5", "Retry", "\u518D\u8A66\u884C"],
    noteTagsExample: ["\u7F8E\u98DF, \u65C5\u884C, \u653B\u7565", "Food, Travel, Guides", "\u30B0\u30EB\u30E1, \u65C5\u884C, \u30AC\u30A4\u30C9"],
    series: ["\u7CFB\u5217", "Series", "\u7CFB\u5217"],
    series1: ["\u7CFB\u5217 1", "Series 1", "\u7CFB\u5217 1"],
    categoryA: ["\u7C7B\u522B A", "Category A", "\u30AB\u30C6\u30B4\u30EA A"],
    categoryB: ["\u7C7B\u522B B", "Category B", "\u30AB\u30C6\u30B4\u30EA B"],
    chapter: ["\u7AE0\u8282 / {{n}}", "CHAPTER / {{n}}", "\u7B2C {{n}} \u7AE0"],
    diaryHeading: ["\u628A\u4ECA\u5929\u7559\u4E0B\u6765", "Keeping Today", "\u4ECA\u65E5\u3092\u6B8B\u3057\u3066"],
    diaryHeadingCaption: ["\u7FFB\u5230\u90A3\u4E00\u5929\uFF0C\u5C31\u53C8\u60F3\u8D77\u4E86\u4F60\u3002", "Turn to that day, and I remember you again.", "\u3042\u306E\u65E5\u306E\u30DA\u30FC\u30B8\u3092\u3081\u304F\u308B\u3068\u3001\u307E\u305F\u541B\u3092\u601D\u3044\u51FA\u3059\u3002"],
    diaryNanaCaption: ["\u6162\u6162\u5199\u4E0B\uFF0C\u4ECA\u5929\u7684\u4E8B\u3002", "Slowly putting today into words.", "\u4ECA\u65E5\u306E\u3053\u3068\u3092\u3001\u3086\u3063\u304F\u308A\u66F8\u304D\u7559\u3081\u3066\u3002"],
    diaryVivianCaption: ["\u8FD9\u4E00\u5929\uFF0C\u503C\u5F97\u8BB0\u4E0B\u6765\u3002", "This day is worth writing down.", "\u3053\u306E\u65E5\u306F\u3001\u66F8\u304D\u6B8B\u3057\u3066\u304A\u304D\u305F\u3044\u3002"],
    notebookEditingHeading: ["\u5199\u5230\u8FD9\u4E00\u683C", "Writing This Panel", "\u3053\u306E\u30B3\u30DE\u306B\u66F8\u3044\u3066"],
    notebookHeading: ["\u8FD8\u6CA1\u8BF4\u5B8C\u7684\u8BDD", "Words Still Unsaid", "\u307E\u3060\u4F1D\u3048\u304D\u308C\u306A\u3044\u8A00\u8449"],
    notebookHeadingCaption: ["\u7075\u611F\u3001\u6458\u5F55\uFF0C\u8FD8\u6709\u60F3\u7559\u4E0B\u7684\u7247\u6BB5\u3002", "Ideas, excerpts, and moments to keep.", "\u3072\u3089\u3081\u304D\u3001\u629C\u304D\u66F8\u304D\u3001\u6B8B\u3057\u3066\u304A\u304D\u305F\u3044\u3072\u3068\u3053\u307E\u3002"],
    plannerHeading: ["\u7EA6\u597D\u4E0B\u4E00\u4EF6\u4E8B", "Our Next Promise", "\u6B21\u306E\u7D04\u675F"],
    reminderHeading: ["\u5230\u4E86\u90A3\u65F6\uFF0C\u63D0\u9192\u6211", "Remind Me When the Time Comes", "\u305D\u306E\u6642\u306B\u306A\u3063\u305F\u3089\u3001\u6559\u3048\u3066"],
    plannerCaption: ["\u5E73\u5E38\u7684\u65E5\u5B50\u91CC\uFF0C\u8FD8\u6709\u60F3\u4E00\u8D77\u505A\u7684\u4E8B\u3002", "There are still things to do together in our ordinary days.", "\u3044\u3064\u3082\u306E\u65E5\u3005\u306B\u3082\u3001\u4E00\u7DD2\u306B\u3084\u308A\u305F\u3044\u3053\u3068\u304C\u3042\u308B\u3002"],
    reminderCaption: ["\u522B\u8BA9\u5C0F\u5C0F\u7684\u7EA6\u5B9A\uFF0C\u6084\u6084\u9519\u8FC7\u3002", "Don't let little promises quietly slip away.", "\u5C0F\u3055\u306A\u7D04\u675F\u3092\u3001\u305D\u3063\u3068\u9003\u3055\u306A\u3044\u3088\u3046\u306B\u3002"],
    chapterArt: ["\u7AE0\u8282\u753B\u9762", "Chapter Artwork", "\u7AE0\u306E\u30A4\u30E9\u30B9\u30C8"],
    planStart: ["\u60F3\u505A\u7684\u4E8B\uFF0C\u5C31\u4ECE\u8FD9\u91CC\u5F00\u59CB\u3002", "What we want to do begins here.", "\u3084\u308A\u305F\u3044\u3053\u3068\u306F\u3001\u3053\u3053\u304B\u3089\u59CB\u307E\u308B\u3002"],
    reminderStart: ["\u5230\u4E86\u65F6\u95F4\uFF0C\u518D\u53EB\u4F60\u4E00\u58F0\u3002", "I'll call you when the time comes.", "\u6642\u9593\u304C\u6765\u305F\u3089\u3001\u307E\u305F\u58F0\u3092\u304B\u3051\u308B\u306D\u3002"],
    planSlow: ["\u4E00\u4EF6\u4E00\u4EF6\uFF0C\u6162\u6162\u5B8C\u6210\u3002", "One thing at a time, slowly.", "\u3072\u3068\u3064\u305A\u3064\u3001\u3086\u3063\u304F\u308A\u53F6\u3048\u3066\u3002"],
    reminderCare: ["\u6709\u4E9B\u4E8B\uFF0C\u503C\u5F97\u60E6\u8BB0\u3002", "Some things are worth remembering.", "\u6C17\u306B\u304B\u3051\u3066\u304A\u304D\u305F\u3044\u3053\u3068\u304C\u3042\u308B\u3002"],
    profileEyebrow: ["\u4E2A\u4EBA\u753B\u50CF", "Personal Profile", "\u30D7\u30ED\u30D5\u30A3\u30FC\u30EB"],
    profileDescription: ["{{name}} \u4ECE\u76F8\u5904\u4E2D\u6162\u6162\u8BA4\u8BC6\u7684\u4F60\u3002\u53CC\u51FB\u5B57\u6BB5\u5373\u53EF\u4FEE\u6539\uFF0C\u9501\u5B9A\u540E\u4E0D\u4F1A\u88AB\u81EA\u52A8\u8986\u76D6\u3002", "What {{name}} learns about you over time. Double-click a field to edit it; locked fields will not be overwritten automatically.", "{{name}} \u304C\u65E5\u3005\u306E\u4F1A\u8A71\u304B\u3089\u77E5\u3063\u3066\u3044\u304F\u541B\u306E\u3053\u3068\u3002\u9805\u76EE\u3092\u30C0\u30D6\u30EB\u30AF\u30EA\u30C3\u30AF\u3057\u3066\u7DE8\u96C6\u3067\u304D\u307E\u3059\u3002\u30ED\u30C3\u30AF\u3059\u308B\u3068\u81EA\u52D5\u66F4\u65B0\u3055\u308C\u307E\u305B\u3093\u3002"],
    profileOverview: ["\u753B\u50CF\u6982\u89C8", "Profile Overview", "\u30D7\u30ED\u30D5\u30A3\u30FC\u30EB\u306E\u6982\u8981"],
    profileKnown: ["\u5DF2\u4E86\u89E3\u7684\u8D44\u6599", "Known Information", "\u5206\u304B\u3063\u3066\u3044\u308B\u60C5\u5831"],
    profileFacts: ["\u8865\u5145\u4E8B\u5B9E", "Additional Facts", "\u8FFD\u52A0\u306E\u4E8B\u5B9F"],
    profileClues: ["\u8FD1\u671F\u7EBF\u7D22", "Recent Clues", "\u6700\u8FD1\u306E\u624B\u304C\u304B\u308A"],
    justNow: ["\u521A\u521A", "Just now", "\u305F\u3063\u305F\u4ECA"],
    readFailed: ["\u52A0\u8F7D\u5931\u8D25", "Failed to load", "\u8AAD\u307F\u8FBC\u307F\u306B\u5931\u6557\u3057\u307E\u3057\u305F"],
    clearSearch: ["\u6E05\u9664\u641C\u7D22", "Clear Search", "\u691C\u7D22\u3092\u30AF\u30EA\u30A2"],
    memoryFacts: ["\u957F\u671F\u8BB0\u5FC6", "Long-term Memories", "\u9577\u671F\u8A18\u61B6"],
    memoryFactSubtitle: ["\u4E8B\u5B9E\u3001\u504F\u597D\u548C\u7EA6\u5B9A", "Facts, preferences and promises", "\u4E8B\u5B9F\u30FB\u597D\u307F\u30FB\u7D04\u675F"],
    memoryEvents: ["\u5171\u540C\u7ECF\u5386", "Shared Experiences", "\u3075\u305F\u308A\u306E\u7D4C\u9A13"],
    memoryEventSubtitle: ["\u8DE8\u4F1A\u8BDD\u7684\u4E8B\u4EF6\u4E0E\u8FDB\u5C55", "Events and progress across conversations", "\u4F1A\u8A71\u3092\u307E\u305F\u3050\u51FA\u6765\u4E8B\u3068\u9032\u5C55"],
    memoryConversations: ["\u5BF9\u8BDD\u8BB0\u5F55", "Conversations", "\u4F1A\u8A71\u8A18\u9332"],
    memoryConversationSubtitle: ["\u6BCF\u6B21\u4EA4\u6D41\u7684\u539F\u59CB\u53D1\u8A00", "Original messages from each exchange", "\u305D\u308C\u305E\u308C\u306E\u4F1A\u8A71\u306E\u5143\u306E\u767A\u8A00"],
    memoryProfile: ["\u7528\u6237\u753B\u50CF", "About You", "\u30E6\u30FC\u30B6\u30FC\u30D7\u30ED\u30D5\u30A3\u30FC\u30EB"],
    memoryProfileSubtitle: ["\u5173\u4E8E\u4F60\u7684\u4E86\u89E3", "What we know about you", "\u541B\u306B\u3064\u3044\u3066\u77E5\u3063\u3066\u3044\u308B\u3053\u3068"],
    memoryTimeUnknown: ["\u65F6\u95F4\u672A\u77E5", "Time unknown", "\u6642\u523B\u4E0D\u660E"],
    memoryPromises: ["\u7EA6\u5B9A\u4E0E\u5F85\u529E", "Promises & Tasks", "\u7D04\u675F\u3068\u3084\u308B\u3053\u3068"],
    memoryPreference: ["\u504F\u597D", "Preferences", "\u597D\u307F"],
    memoryRelation: ["\u5173\u7CFB", "Relationships", "\u95A2\u4FC2"],
    memoryAbout: ["\u5173\u4E8E\u4F60", "About You", "\u541B\u306B\u3064\u3044\u3066"],
    memoryTopics: ["\u5171\u540C\u8BDD\u9898", "Shared Topics", "\u5171\u901A\u306E\u8A71\u984C"],
    memorySummary: ["\u4F1A\u8BDD\u6458\u8981", "Conversation Summary", "\u4F1A\u8A71\u306E\u8981\u7D04"],
    memoryData: ["\u4E8B\u5B9E\u4E0E\u8D44\u6599", "Facts & Information", "\u4E8B\u5B9F\u3068\u8CC7\u6599"],
    memoryIdentity: ["\u8EAB\u4EFD", "Identity", "\u57FA\u672C\u60C5\u5831"],
    memoryProject: ["\u9879\u76EE", "Projects", "\u30D7\u30ED\u30B8\u30A7\u30AF\u30C8"],
    memoryHealth: ["\u5065\u5EB7", "Health", "\u5065\u5EB7"],
    memoryReference: ["\u53C2\u8003", "References", "\u53C2\u8003"],
    memoryKnowledge: ["\u77E5\u8BC6", "Knowledge", "\u77E5\u8B58"],
    memoryNoSummary: ["\u8FD9\u6B21\u4EA4\u6D41\u65E0\u9700\u989D\u5916\u6458\u8981\uFF0C\u539F\u59CB\u6D88\u606F\u5DF2\u4FDD\u7559\u3002", "No extra summary is needed. The original messages are preserved.", "\u8FFD\u52A0\u306E\u8981\u7D04\u306F\u4E0D\u8981\u3067\u3059\u3002\u5143\u306E\u30E1\u30C3\u30BB\u30FC\u30B8\u306F\u4FDD\u5B58\u3055\u308C\u3066\u3044\u307E\u3059\u3002"],
    memoryPartial: ["\u5DF2\u4FDD\u5B58\u9636\u6BB5\u6458\u8981\uFF0C\u5269\u4F59\u5185\u5BB9\u53EF\u7EE7\u7EED\u6574\u7406\u3002", "A partial summary is saved. You can continue with the rest.", "\u9014\u4E2D\u306E\u8981\u7D04\u3092\u4FDD\u5B58\u3057\u307E\u3057\u305F\u3002\u6B8B\u308A\u306E\u6574\u7406\u3092\u7D9A\u3051\u3089\u308C\u307E\u3059\u3002"],
    memoryUpdated: ["\u4F1A\u8BDD\u6458\u8981\u5DF2\u66F4\u65B0\uFF0C\u6709\u4F9D\u636E\u7684\u4E8B\u4EF6\u8FDB\u5C55\u4F1A\u8FDB\u5165\u5171\u540C\u7ECF\u5386\u3002", "The summary is updated. Supported event progress appears in Shared Experiences.", "\u4F1A\u8A71\u306E\u8981\u7D04\u3092\u66F4\u65B0\u3057\u307E\u3057\u305F\u3002\u6839\u62E0\u306E\u3042\u308B\u9032\u5C55\u306F\u300C\u3075\u305F\u308A\u306E\u7D4C\u9A13\u300D\u306B\u8868\u793A\u3055\u308C\u307E\u3059\u3002"],
    memoryHeading: ["\u8BB0\u5F97\u7684\u4E8B", "Things We Remember", "\u899A\u3048\u3066\u3044\u308B\u3053\u3068"],
    memoryCaption: ["\u90A3\u5929\u8BF4\u8FC7\u7684\u8BDD\uFF0C\u8FD8\u7559\u5728\u8FD9\u91CC\u3002", "The words from that day are still here.", "\u3042\u306E\u65E5\u4EA4\u308F\u3057\u305F\u8A00\u8449\u306F\u3001\u3053\u3053\u306B\u6B8B\u3063\u3066\u3044\u308B\u3002"],
    memoryIntro: ["\u539F\u59CB\u4EA4\u6D41\u3001\u4F1A\u8BDD\u8109\u7EDC\uFF0C\u4EE5\u53CA\u6709\u4F9D\u636E\u7684\u4E8B\u5B9E\u4E0E\u7EA6\u5B9A\u3002", "Original exchanges, their context, and supported facts and promises.", "\u5143\u306E\u4F1A\u8A71\u3001\u305D\u306E\u6D41\u308C\u3001\u6839\u62E0\u306E\u3042\u308B\u4E8B\u5B9F\u3068\u7D04\u675F\u3002"],
    memoryCategories: ["\u8BB0\u5FC6\u5206\u7C7B", "Memory Categories", "\u8A18\u61B6\u306E\u5206\u985E"],
    memoryGroup: ["\u7EC4", "conversations", "\u4EF6\u306E\u4F1A\u8A71"],
    memoryItem: ["\u6761", "items", "\u4EF6"],
    memorySearchPlaceholder: ["\u641C\u7D22\u8FD9\u91CC\u7684\u8BB0\u5FC6", "Search these memories", "\u3053\u3053\u306B\u3042\u308B\u8A18\u61B6\u3092\u691C\u7D22"],
    memorySearch: ["\u641C\u7D22\u8BB0\u5FC6", "Search Memories", "\u8A18\u61B6\u3092\u691C\u7D22"],
    memoryRecentNote: ["\u6BCF\u5F20\u5361\u7247\u4FDD\u7559\u4E00\u6B21\u4EA4\u6D41\u7684\u539F\u59CB\u6D88\u606F\u548C\u4F1A\u8BDD\u6458\u8981\uFF1B\u5177\u4F53\u4E8B\u4EF6\u7684\u8FDB\u5C55\u5355\u72EC\u6C47\u5165\u5171\u540C\u7ECF\u5386\u3002", "Each card preserves an exchange and its summary. Event progress is collected separately in Shared Experiences.", "\u5404\u30AB\u30FC\u30C9\u306F\u5143\u306E\u4F1A\u8A71\u3068\u8981\u7D04\u3092\u4FDD\u5B58\u3057\u307E\u3059\u3002\u51FA\u6765\u4E8B\u306E\u9032\u5C55\u306F\u300C\u3075\u305F\u308A\u306E\u7D4C\u9A13\u300D\u306B\u307E\u3068\u3081\u3089\u308C\u307E\u3059\u3002"],
    memoryEventNote: ["\u540C\u4E00\u4E8B\u4EF6\u5728\u4E0D\u540C\u4F1A\u8BDD\u4E2D\u7684\u8BA1\u5212\u3001\u8FDB\u5C55\u4E0E\u7ED3\u679C\u4E32\u6210\u4E00\u6761\u65F6\u95F4\u7EBF\u3002\u65F6\u95F4\u8868\u793A\u539F\u8BDD\u8BB0\u5F55\u65F6\u95F4\uFF0C\u8BA1\u5212\u4E0D\u4EE3\u8868\u5DF2\u7ECF\u53D1\u751F\u3002", "Plans, progress and outcomes from different conversations form a timeline. Times refer to the original records; plans have not necessarily happened.", "\u8907\u6570\u306E\u4F1A\u8A71\u306B\u3042\u308B\u8A08\u753B\u30FB\u9032\u5C55\u30FB\u7D50\u679C\u3092\u6642\u7CFB\u5217\u306B\u307E\u3068\u3081\u307E\u3059\u3002\u6642\u523B\u306F\u5143\u306E\u767A\u8A00\u306E\u8A18\u9332\u6642\u523B\u3067\u3059\u3002\u8A08\u753B\u306F\u5B9F\u884C\u6E08\u307F\u3092\u610F\u5473\u3057\u307E\u305B\u3093\u3002"],
    memoryLoading: ["\u6B63\u5728\u8BFB\u53D6\u8BB0\u5FC6\u2026", "Loading memories\u2026", "\u8A18\u61B6\u3092\u8AAD\u307F\u8FBC\u3093\u3067\u3044\u307E\u3059\u2026"],
    memoryNoMatch: ["\u6CA1\u6709\u627E\u5230\u5339\u914D\u7684\u8BB0\u5FC6", "No matching memories", "\u4E00\u81F4\u3059\u308B\u8A18\u61B6\u304C\u898B\u3064\u304B\u308A\u307E\u305B\u3093"],
    memoryTryKeywords: ["\u8BD5\u8BD5\u5176\u4ED6\u5173\u952E\u8BCD", "Try other keywords", "\u5225\u306E\u30AD\u30FC\u30EF\u30FC\u30C9\u3092\u8A66\u3057\u3066\u304F\u3060\u3055\u3044"],
    memoryEmptyEvents: ["\u6574\u7406\u5BF9\u8BDD\u540E\uFF0C\u6709\u539F\u8BDD\u4F9D\u636E\u7684\u4E8B\u4EF6\u4F1A\u51FA\u73B0\u5728\u8FD9\u91CC\uFF1B\u666E\u901A\u804A\u5929\u4E0D\u4F1A\u6210\u4E3A\u4E8B\u4EF6\u3002", "After organizing a conversation, supported events appear here. Ordinary chat is not treated as an event.", "\u4F1A\u8A71\u3092\u6574\u7406\u3059\u308B\u3068\u3001\u5143\u306E\u767A\u8A00\u306B\u88CF\u3065\u3051\u3089\u308C\u305F\u51FA\u6765\u4E8B\u304C\u8868\u793A\u3055\u308C\u307E\u3059\u3002\u65E5\u5E38\u306E\u96D1\u8AC7\u306F\u51FA\u6765\u4E8B\u306B\u306F\u306A\u308A\u307E\u305B\u3093\u3002"],
    memoryEmptyHint: ["\u6709\u65B0\u7684\u4EA4\u6D41\u65F6\uFF0C\u8FD9\u91CC\u4F1A\u6162\u6162\u4E30\u5BCC\u8D77\u6765", "New conversations will gradually fill this space", "\u65B0\u3057\u3044\u4F1A\u8A71\u3068\u3068\u3082\u306B\u3001\u3053\u3053\u3082\u5C11\u3057\u305A\u3064\u8C4A\u304B\u306B\u306A\u308A\u307E\u3059"],
    memoryRecorded: ["\u8BB0\u5F55\u4E8E ", "Recorded ", "\u8A18\u9332\u6642\u523B "],
    memoryPlanned: ["\u8BA1\u5212\u4E0E\u7EA6\u5B9A", "Plans & Promises", "\u8A08\u753B\u3068\u7D04\u675F"],
    memoryStarted: ["\u5F00\u59CB", "Started", "\u958B\u59CB"],
    memoryProgress: ["\u8FDB\u5C55", "Progress", "\u9032\u5C55"],
    memoryComplete: ["\u5DF2\u5B8C\u6210", "Completed", "\u5B8C\u4E86"],
    memoryCancel: ["\u5DF2\u53D6\u6D88", "Cancelled", "\u30AD\u30E3\u30F3\u30BB\u30EB"],
    memoryEvidence: ["\u539F\u8BDD\u4F9D\u636E", "Original Evidence", "\u5143\u306E\u767A\u8A00\u306B\u3088\u308B\u6839\u62E0"],
    memorySource: ["\u67E5\u770B\u6765\u6E90\u4F1A\u8BDD", "View Source Conversation", "\u5143\u306E\u4F1A\u8A71\u3092\u898B\u308B"],
    memoryCollapse: ["\u6536\u8D77", "Collapse", "\u6298\u308A\u305F\u305F\u3080"],
    memoryOrganizing: ["\u6B63\u5728\u6574\u7406\u2026", "Organizing\u2026", "\u6574\u7406\u4E2D\u2026"],
    memoryContinue: ["\u7EE7\u7EED\u6574\u7406", "Continue Organizing", "\u6574\u7406\u3092\u7D9A\u3051\u308B"],
    memoryUpdateSummary: ["\u66F4\u65B0\u6458\u8981", "Update Summary", "\u8981\u7D04\u3092\u66F4\u65B0"],
    memoryOrganize: ["\u6574\u7406\u8FD9\u6B21\u5BF9\u8BDD", "Organize This Conversation", "\u3053\u306E\u4F1A\u8A71\u3092\u6574\u7406"],
    memoryOriginal: ["\u67E5\u770B\u539F\u59CB\u4F1A\u8BDD", "View Original Conversation", "\u5143\u306E\u4F1A\u8A71\u3092\u898B\u308B"],
    memoryViewSummary: ["\u67E5\u770B\u4F1A\u8BDD\u6458\u8981", "View Summary", "\u4F1A\u8A71\u306E\u8981\u7D04\u3092\u898B\u308B"],
    memoryKept: ["\u7559\u4E0B\u7684\u8BB0\u5FC6", "Memories Kept", "\u6B8B\u3055\u308C\u305F\u8A18\u61B6"],
    memoryViewImage: ["\u67E5\u770B\u539F\u56FE", "View Original Image", "\u5143\u306E\u753B\u50CF\u3092\u898B\u308B"],
    memoryImageMissing: ["\u539F\u56FE\u6587\u4EF6\u5DF2\u4E0D\u5728\u672C\u5730\uFF0C\u53EF\u80FD\u5DF2\u88AB\u6E05\u7406", "The original image is gone from disk", "\u5143\u753B\u50CF\u304C\u30ED\u30FC\u30AB\u30EB\u306B\u6B8B\u3063\u3066\u3044\u307E\u305B\u3093"],
    memoryQuote: ["\u6765\u81EA\u539F\u8BDD", "From the Original Words", "\u5143\u306E\u767A\u8A00\u3088\u308A"],
    memoryFollowUp: ["\u5F85\u8DDF\u8FDB", "Follow Up", "\u8981\u30D5\u30A9\u30ED\u30FC"],
    memoryView: ["\u67E5\u770B\u4F1A\u8BDD", "View Conversation", "\u4F1A\u8A71\u3092\u898B\u308B"],
    memoryExtracted: ["\u539F\u8BDD\u63D0\u53D6", "Extracted from Original Words", "\u5143\u306E\u767A\u8A00\u304B\u3089\u62BD\u51FA"],
    memoryNoEvidence: ["\u672A\u6807\u6CE8\u8BC1\u636E", "No Evidence Marked", "\u6839\u62E0\u306E\u6307\u5B9A\u306A\u3057"],
    memoryImportant: ["\u91CD\u8981\u4E8B\u4EF6", "Important Event", "\u91CD\u8981\u306A\u51FA\u6765\u4E8B"],
    memoryProfileUnderstanding: ["{{name}} \u7684\u4E86\u89E3", "{{name}}\u2019s Understanding", "{{name}} \u304C\u77E5\u3063\u3066\u3044\u308B\u3053\u3068"],
    memoryEmpty: ["\u8FD8\u6CA1\u6709{{name}}\u7684\u8BB0\u5F55", "No {{name}} records yet", "{{name}}\u306E\u8A18\u9332\u306F\u307E\u3060\u3042\u308A\u307E\u305B\u3093"],
    memoryEventCounts: ["{{conversations}} \u6B21\u4F1A\u8BDD \xB7 {{progress}} \u6761\u8FDB\u5C55", "{{conversations}} conversations \xB7 {{progress}} updates", "{{conversations}} \u4EF6\u306E\u4F1A\u8A71 \xB7 {{progress}} \u4EF6\u306E\u9032\u5C55"],
    memoryMessages: ["{{n}} \u6761\u6D88\u606F", "{{n}} messages", "{{n}} \u4EF6\u306E\u30E1\u30C3\u30BB\u30FC\u30B8"],
    memoryExpandMessages: ["\u5C55\u5F00\u5168\u90E8 {{n}} \u6761\u6D88\u606F", "Show all {{n}} messages", "\u5168 {{n}} \u4EF6\u306E\u30E1\u30C3\u30BB\u30FC\u30B8\u3092\u8868\u793A"],
    memoryMoreEvidence: ["\u66F4\u591A\u539F\u8BDD\u8BC1\u636E \xB7 {{n}} \u6761", "More original evidence \xB7 {{n}} quotes", "\u5143\u306E\u767A\u8A00\u306E\u6839\u62E0\u3092\u3055\u3089\u306B\u8868\u793A \xB7 {{n}} \u4EF6"],
    memoryPartialCount: ["\u9636\u6BB5\u6458\u8981 \xB7 {{done}}/{{total}} \u6BB5", "Partial summary \xB7 {{done}}/{{total}} parts", "\u9014\u4E2D\u306E\u8981\u7D04 \xB7 {{done}}/{{total}} \u90E8\u5206"],
    memoryCoverage: ["\u8986\u76D6 {{n}} \u6761\u539F\u6D88\u606F", "Covers {{n}} original messages", "\u5143\u306E\u30E1\u30C3\u30BB\u30FC\u30B8 {{n}} \u4EF6\u3092\u5BFE\u8C61"],
    "alt_morning-curtains": ["\u6668\u5149\u900F\u8FC7\u7A97\u5E18\uFF0C\u843D\u5728\u5367\u5BA4\u5730\u677F\u4E0A", "Morning light falls through the curtains onto the bedroom floor", "\u671D\u306E\u5149\u304C\u30AB\u30FC\u30C6\u30F3\u3092\u901A\u308A\u3001\u5BDD\u5BA4\u306E\u5E8A\u306B\u843D\u3061\u308B"],
    "alt_morning-alarm": ["\u6E05\u6668\u4E03\u70B9\uFF0C\u5E8A\u5934\u7684\u7C89\u8272\u53CC\u94C3\u95F9\u949F", "A pink twin-bell alarm clock at seven in the morning", "\u671D\u4E03\u6642\u3001\u6795\u5143\u306E\u30D4\u30F3\u30AF\u306E\u30D9\u30EB\u4ED8\u304D\u76EE\u899A\u307E\u3057\u6642\u8A08"],
    "alt_morning-waking": ["Vivian \u4FA7\u8EBA\u5728\u6795\u5934\u4E0A\uFF0C\u4F38\u624B\u5173\u95ED\u95F9\u949F", "Vivian reaches from her pillow to turn off the alarm", "\u6795\u306B\u6A2A\u305F\u308F\u308B Vivian \u304C\u3001\u76EE\u899A\u307E\u3057\u6642\u8A08\u3092\u6B62\u3081\u3088\u3046\u3068\u624B\u3092\u4F38\u3070\u3059"],
    "alt_waiting-v": ["Vivian \u5728\u7A97\u8FB9\u51FA\u795E\uFF0C\u6D45\u767D\u7684\u53D1\u4E1D\u6620\u7740\u5929\u5149", "Vivian gazes out the window, pale hair catching the daylight", "\u7A93\u8FBA\u3067\u7269\u601D\u3044\u306B\u3075\u3051\u308B Vivian\u3002\u6DE1\u3044\u9AEA\u306B\u5149\u304C\u6620\u308B"],
    "alt_duo-home": ["Nana \u5728\u660E\u4EAE\u7684\u56DE\u5ECA\u91CC\u6E29\u67D4\u56DE\u671B", "Nana looks back gently in a bright hallway", "\u660E\u308B\u3044\u5ECA\u4E0B\u3067\u512A\u3057\u304F\u632F\u308A\u8FD4\u308B Nana"],
    "alt_platform-light": ["Vivian \u521A\u4E0B\u7535\u8F66\uFF0C\u5E26\u7740\u6536\u8D77\u7684\u4F1E\u5BFB\u627E Nana", "Vivian steps off the train, looking for Nana with a folded umbrella", "\u96FB\u8ECA\u3092\u964D\u308A\u305F Vivian \u304C\u3001\u9589\u3058\u305F\u5098\u3092\u624B\u306B Nana \u3092\u63A2\u3059"],
    "alt_rain-soaked-detail": ["Nana \u6CA1\u6709\u6491\u4F1E\uFF0C\u53CC\u624B\u6525\u4F4F\u88AB\u96E8\u6DCB\u6E7F\u7684\u88D9\u6446", "Without an umbrella, Nana holds the hem of her rain-soaked skirt", "\u5098\u3092\u5DEE\u3055\u305A\u3001\u96E8\u306B\u6FE1\u308C\u305F\u30B9\u30AB\u30FC\u30C8\u306E\u88FE\u3092\u4E21\u624B\u3067\u63E1\u308B Nana"],
    "alt_waiting-n": ["Nana \u5728\u95E8\u8FB9\u7B49\u5019\uFF0C\u53CC\u624B\u8F7B\u8F7B\u6525\u4F4F\u88D9\u6446", "Nana waits by the door, lightly holding her skirt", "\u6249\u306E\u305D\u3070\u3067\u3001\u305D\u3063\u3068\u30B9\u30AB\u30FC\u30C8\u3092\u63E1\u308A\u306A\u304C\u3089\u5F85\u3064 Nana"],
    "alt_emotion-n": ["Nana \u4F1E\u4E0B\u6E29\u67D4\u7684\u4FA7\u8138", "Nana's gentle profile beneath an umbrella", "\u5098\u306E\u4E0B\u306E Nana \u306E\u512A\u3057\u3044\u6A2A\u9854"],
    "alt_emotion-v": ["Vivian \u8F7B\u629A\u80F8\u524D\uFF0C\u7559\u610F\u81EA\u5DF1\u7684\u5FC3\u60C5", "Vivian rests a hand on her chest, listening to her feelings", "\u80F8\u306B\u305D\u3063\u3068\u624B\u3092\u5F53\u3066\u3001\u81EA\u5206\u306E\u6C17\u6301\u3061\u3092\u611F\u3058\u308B Vivian"],
    "alt_cafe-exterior": ["\u96E8\u5929\u7684\u8857\u9053\u4E0A\uFF0C\u5496\u5561\u5385\u900F\u7740\u6696\u5149", "A caf\xE9 glows warmly on a rainy street", "\u96E8\u306E\u901A\u308A\u306B\u3001\u30AB\u30D5\u30A7\u306E\u6E29\u304B\u306A\u660E\u304B\u308A\u304C\u3053\u307C\u308C\u308B"],
    "alt_cafe-interior": ["Vivian \u548C Nana \u5750\u5728\u5496\u5561\u5385\u7A97\u8FB9\uFF0C\u5206\u4EAB\u684C\u4E0A\u7684\u65E5\u8BB0", "Vivian and Nana share a journal beside the caf\xE9 window", "\u30AB\u30D5\u30A7\u306E\u7A93\u8FBA\u3067\u3001\u65E5\u8A18\u3092\u4E00\u7DD2\u306B\u8AAD\u3080 Vivian \u3068 Nana"],
    "alt_door-smile": ["Nana \u4E0E Vivian \u5171\u63E1\u4F1E\u67C4\uFF0C\u8F7B\u63E1\u5979\u7684\u624B\u8155", "Nana shares the umbrella handle with Vivian and gently holds her wrist", "Vivian \u3068\u5098\u3092\u6301\u3061\u3001\u305D\u3063\u3068\u624B\u9996\u306B\u89E6\u308C\u308B Nana"],
    "alt_letter-desk": ["\u644A\u5F00\u7684\u624B\u8BB0\u3001\u4FE1\u7B3A\u4E0E\u4E24\u4EBA\u7684\u5C0F\u7269\u4EF6", "An open journal, letters and their little keepsakes", "\u958B\u3044\u305F\u624B\u5E33\u3001\u4FBF\u7B8B\u3001\u3075\u305F\u308A\u306E\u5C0F\u3055\u306A\u6301\u3061\u7269"],
    "alt_line-v": ["Vivian \u5750\u5728\u5E8A\u8FB9\uFF0C\u4F4E\u5934\u8BFB\u624B\u673A\u6D88\u606F", "Vivian reads a message while sitting on her bed", "\u30D9\u30C3\u30C9\u306B\u5EA7\u308A\u3001\u30B9\u30DE\u30FC\u30C8\u30D5\u30A9\u30F3\u306E\u30E1\u30C3\u30BB\u30FC\u30B8\u3092\u8AAD\u3080 Vivian"],
    "alt_line-phone": ["\u624B\u673A\u6682\u65F6\u7184\u5C4F\uFF0C\u60AC\u6D6E\u6216\u81EA\u52A8\u6E38\u89C8\u65F6\u4EAE\u8D77", "The phone lights up on hover or during the automatic tour", "\u30B9\u30DE\u30FC\u30C8\u30D5\u30A9\u30F3\u306F\u3001\u30AB\u30FC\u30BD\u30EB\u3092\u91CD\u306D\u308B\u304B\u81EA\u52D5\u95B2\u89A7\u4E2D\u306B\u70B9\u706F\u3059\u308B"],
    "alt_daily-arrival": ["\u96E8\u540E\u7684\u7AD9\u53F0\u4E0E\u9A76\u6765\u7684\u7535\u8F66", "A train arrives at the platform after the rain", "\u96E8\u4E0A\u304C\u308A\u306E\u30DB\u30FC\u30E0\u306B\u5230\u7740\u3059\u308B\u96FB\u8ECA"],
    "alt_daily-straps": ["\u8F66\u53A2\u540A\u73AF\u548C\u7A97\u5916\u7684\u6CB3\u5CB8", "Train straps and the riverbank outside", "\u8ECA\u5185\u306E\u3064\u308A\u9769\u3068\u3001\u7A93\u306E\u5916\u306E\u5DDD\u5CB8"],
    "alt_daily-pole": ["Vivian \u6276\u7740\u8F66\u53A2\u7ACB\u6746\u8F7B\u8F7B\u5FAE\u7B11", "Vivian smiles softly while holding a train pole", "\u8ECA\u5185\u306E\u624B\u3059\u308A\u306B\u3064\u304B\u307E\u308A\u3001\u305D\u3063\u3068\u5FAE\u7B11\u3080 Vivian"],
    "alt_daily-tickets": ["\u7A97\u8FB9\u7684\u8F66\u7968\u3001\u6C34\u74F6\u548C\u6536\u8D77\u7684\u96E8\u4F1E", "Tickets, a water bottle and a folded umbrella beside the window", "\u7A93\u8FBA\u306E\u5207\u7B26\u3001\u6C34\u306E\u30DC\u30C8\u30EB\u3001\u9589\u3058\u305F\u5098"],
    "alt_skirt-full": ["Vivian \u7684\u6307\u5C16\u8F7B\u8F7B\u6525\u4F4F\u88D9\u6446", "Vivian's fingers gently gather her skirt", "\u305D\u3063\u3068\u30B9\u30AB\u30FC\u30C8\u306E\u88FE\u3092\u63E1\u308B Vivian \u306E\u6307\u5148"],
    "alt_skirt-hand": ["Vivian \u7684\u6307\u5C16\u8F7B\u8F7B\u6525\u4F4F\u88D9\u6446", "A close-up of Vivian's fingers gently gathering her skirt", "\u30B9\u30AB\u30FC\u30C8\u306E\u88FE\u3092\u305D\u3063\u3068\u63E1\u308B Vivian \u306E\u6307\u5148\u306E\u30A2\u30C3\u30D7"],
    "alt_hold-n": ["Vivian \u4E0E Nana \u5149\u817F\u80CC\u9760\u80CC\u5750\u5728\u96E8\u4E2D\uFF0CVivian \u8F7B\u63E1\u88D9\u6446", "Vivian and Nana sit back-to-back with bare legs in the rain, Vivian gently holding her skirt", "\u96E8\u306E\u4E2D\u3001\u30B9\u30C8\u30C3\u30AD\u30F3\u30B0\u3092\u5C65\u304B\u305A\u80CC\u4E2D\u5408\u308F\u305B\u306B\u5EA7\u308B Vivian \u3068 Nana\u3002Vivian \u306F\u305D\u3063\u3068\u30B9\u30AB\u30FC\u30C8\u3092\u63E1\u308B"],
    title0: ["Vivian & Nana", "Vivian & Nana", "Vivian & Nana"],
    title3: ["\u628A\u4ECA\u5929\u7559\u4E0B", "Keeping Today", "\u4ECA\u65E5\u3092\u6B8B\u3057\u3066"],
    title5: ["\u7EA6\u5B9A", "Promises", "\u7D04\u675F"],
    title7: ["\u7559\u4E0B\u7684\u8DB3\u8FF9", "Footprints We Leave", "\u6B8B\u3057\u305F\u8DB3\u8DE1"],
    contents: ["CONTENT", "CONTENT", "CONTENT"],
    vivianThoughtLabel: ["Vivian \u5F53\u524D\u60F3\u6CD5", "Vivian\u2019s current thought", "Vivian \u306E\u4ECA\u306E\u60F3\u3044"],
    nanaThoughtLabel: ["Nana \u5F53\u524D\u60F3\u6CD5", "Nana\u2019s current thought", "Nana \u306E\u4ECA\u306E\u60F3\u3044"],
    browse: ["\u968F\u624B\u7FFB\u9605", "Browse", "\u6C17\u307E\u307E\u306B\u3081\u304F\u308B"],
    destinations: ["\u5185\u5BB9\u5B9A\u4F4D", "Jump to Content", "\u5185\u5BB9\u3078\u79FB\u52D5"],
    historicalRecord: ["\u5386\u53F2\u8BB0\u5F55", "History", "\u904E\u53BB\u306E\u8A18\u9332"],
    historicalConversation: ["\u5386\u53F2\u5BF9\u8BDD", "Past conversation", "\u904E\u53BB\u306E\u4F1A\u8A71"],
    sharedExperience: ["\u5171\u540C\u7ECF\u5386", "Shared experience", "\u4E00\u7DD2\u306E\u51FA\u6765\u4E8B"],
    previous: ["\u4E0A\u4E00\u9875", "Previous Page", "\u524D\u306E\u30DA\u30FC\u30B8"],
    next: ["\u4E0B\u4E00\u9875", "Next Page", "\u6B21\u306E\u30DA\u30FC\u30B8"],
    gesture: ["{{title}}\uFF0C\u5DE6\u53F3\u62D6\u62FD\u6216\u6EDA\u8F6E\u7FFB\u9875", "{{title}}. Drag sideways or scroll to turn pages.", "{{title}}\u3002\u5DE6\u53F3\u306B\u30C9\u30E9\u30C3\u30B0\u3001\u307E\u305F\u306F\u30B9\u30AF\u30ED\u30FC\u30EB\u3067\u30DA\u30FC\u30B8\u3092\u3081\u304F\u308A\u307E\u3059\u3002"],
    refresh: ["\u5237\u65B0\u8BB0\u5FC6", "Refresh Memories", "\u8A18\u61B6\u3092\u66F4\u65B0"],
    expand: ["\u5C55\u5F00{{name}}", "Open {{name}}", "{{name}}\u3092\u958B\u304F"],
    collapse: ["\u6536\u56DE{{name}}", "Close {{name}}", "{{name}}\u3092\u9589\u3058\u308B"],
    panelMemory: ["\u8BB0\u5FC6\u4E0E\u5BF9\u8BDD", "Memories & Conversations", "\u8A18\u61B6\u3068\u4F1A\u8A71"],
    panelNotebook: ["\u7559\u4E0B\u7684\u8DB3\u8FF9", "Footprints We Leave", "\u6B8B\u3057\u305F\u8DB3\u8DE1"],
    panelPlanner: ["\u5C0F\u5C0F\u7684\u7EA6\u5B9A", "Little Promises", "\u5C0F\u3055\u306A\u7D04\u675F"],
    morning: ["\u65E9\u6668", "Morning", "\u671D"],
    heartbeat: ["\u6026\u2026", "Thump\u2026", "\u30C9\u30AD\u2026"],
    online: ["\u5728\u7EBF", "Online", "\u30AA\u30F3\u30E9\u30A4\u30F3"],
    busy: ["\u5FD9\u788C", "Busy", "\u53D6\u308A\u8FBC\u307F\u4E2D"],
    rest: ["\u4F11\u606F", "Resting", "\u4F11\u61A9\u4E2D"],
    offline: ["\u79BB\u7EBF", "Offline", "\u30AA\u30D5\u30E9\u30A4\u30F3"],
    unknown: ["\u72B6\u6001\u672A\u77E5", "Status unknown", "\u72B6\u614B\u4E0D\u660E"],
    seconds: ["{{n}}\u79D2", "{{n}}s", "{{n}}\u79D2"],
    minutes: ["{{n}}\u5206\u949F", "{{n}}m", "{{n}}\u5206"],
    hours: ["{{n}}\u5C0F\u65F6", "{{n}}h", "{{n}}\u6642\u9593"],
    audience: ["\u5BF9 {{name}} \u8BF4", "Speaking to {{name}}", "{{name}}\u306B\u8A71\u3057\u304B\u3051\u3066\u3044\u307E\u3059"]
  };
  var recordTranslations = Object.fromEntries(["zh-CN", "en", "ja"].map((lang, index) => [lang, Object.fromEntries(Object.entries(rows).map(([key, values]) => [key, values[index]]))]));
  function recordLocale(language = "zh-CN") {
    return language.startsWith("ja") ? "ja" : language.startsWith("en") ? "en" : "zh-CN";
  }
  function recordText(key, language = "zh-CN") {
    return recordTranslations[recordLocale(language)][key] ?? key;
  }
  var literalKeys = new Map(Object.entries(recordTranslations["zh-CN"]).map(([key, value]) => [value, key]));

  // src/components/mind-inspector/pages/memoryPresentation.ts
  var speakerName = (value, character) => {
    const key = value.trim().toLowerCase();
    if (key === "i" || key === "me") return characterName(character);
    if (key === "user") return "\u7528\u6237";
    if (isCharacterId(key)) return characterName(key);
    return value.trim();
  };
  var isMemoryFact = (item) => item.metadata?.record_kind === "fact";
  var isMemorySummary = (item) => item.metadata?.record_kind === "session_summary";
  var ongoingTaskOf = (item) => {
    const value = item.metadata?.ongoing_task;
    return item.metadata?.record_kind === "ongoing_task" && value && Array.isArray(value.entries) && typeof value.title === "string" && typeof value.goal === "string" && ["candidate", "active", "paused", "completed", "cancelled"].includes(value.status) ? value : void 0;
  };
  var isVisibleOngoingTask = (item) => !!ongoingTaskOf(item) && !item.consolidated && !["system_seed", "environment_preset"].includes(String(item.metadata?.source ?? ""));
  var ongoingLabel = (value, language = "zh") => {
    const labels = {
      ongoing: ["\u5173\u6CE8\u4E0E\u7EA6\u5B9A", "Interest & agreement", "\u95A2\u5FC3\u3068\u7D04\u675F"],
      candidate: ["\u5019\u9009\u5173\u6CE8", "Possible interest", "\u95A2\u5FC3\u306E\u5019\u88DC"],
      active: ["\u7EF4\u62A4\u4E2D", "Active", "\u7D99\u7D9A\u4E2D"],
      paused: ["\u5DF2\u6682\u505C", "Paused", "\u4E00\u6642\u505C\u6B62"],
      completed: ["\u5DF2\u5B8C\u6210", "Completed", "\u5B8C\u4E86"],
      cancelled: ["\u5DF2\u53D6\u6D88", "Cancelled", "\u53D6\u308A\u6D88\u3057"],
      planned: ["\u8BA1\u5212\u4E2D", "Planned", "\u4E88\u5B9A"],
      in_progress: ["\u8FDB\u884C\u4E2D", "In progress", "\u9032\u884C\u4E2D"],
      dropped: ["\u5DF2\u653E\u4E0B", "Dropped", "\u4E2D\u6B62"],
      recorded: ["\u5DF2\u8BB0\u5F55", "Recorded", "\u8A18\u9332\u6E08\u307F"],
      collection: ["\u5B8C\u6574\u6E05\u5355", "Full collection", "\u5168\u30EA\u30B9\u30C8"]
    };
    return labels[value]?.[language.startsWith("en") ? 1 : language.startsWith("ja") ? 2 : 0] ?? value;
  };
  var ongoingSearchText = (item) => {
    const task = ongoingTaskOf(item);
    return task ? [
      task.title,
      task.goal,
      task.trigger,
      ...task.entries.flatMap((entry) => [entry.label, entry.detail, entry.evidence.quote])
    ].join(" ") : "";
  };
  var isImageObservation = (item) => item.metadata?.kind === "image";
  var isVisibleMemoryFact = (item) => !["system_seed", "environment_preset"].includes(String(item.metadata?.source ?? "")) && !!item.content.trim() && !item.consolidated && !isImageObservation(item) && !["subjective", "observation", "internal"].includes(String(item.metadata?.record_kind ?? "")) && isMemoryFact(item);
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
  function conversationTitle(record, character, language = "zh-CN") {
    const others = record.participants.filter((name2) => name2 !== character);
    const originalNames = others.map((name2) => name2 === "user" ? "\u4F60" : characterName(name2));
    if (record.title !== `\u4E0E${originalNames.join("\u3001")}\u7684\u5BF9\u8BDD`) return record.title;
    const firstTurn = record.turns.reduce((first2, turn) => !first2 || turn.timestamp < first2.timestamp ? turn : first2, void 0);
    const first = firstTurn?.speaker ?? (record.participants.includes("user") ? "user" : record.participants[0] ?? character);
    const participants = [.../* @__PURE__ */ new Set([first, ...record.participants])];
    const name = (person) => person === "user" ? recordText("conversationYou", language) : speakerName(person, character);
    return recordText("conversationParticipants", language).replace("{{first}}", name(first)).replace("{{others}}", participants.filter((person) => person !== first).map(name).join(recordLocale(language) === "en" ? ", " : "\u3001"));
  }
  var turnImage = (turn) => {
    const description = turn.visual_description?.trim();
    if (!description && !turn.image_path) return void 0;
    return { path: turn.image_path, description: description || void 0 };
  };
  var turnText = (turn) => turn.visual_description?.trim() || turn.text;
  var conversationThreads = (records, character, language = "zh-CN") => records.map((record) => ({
    id: record.id,
    title: conversationTitle(record, character, language),
    time: record.ended_at,
    startedAt: record.started_at,
    canonical: true,
    turns: record.turns.map((turn) => ({
      id: turn.id,
      speaker: speakerName(turn.speaker, character),
      audience: speakerName(turn.listener === "all" ? "\u5927\u5BB6" : turn.listener, character),
      text: turnText(turn),
      timestamp: turn.timestamp,
      sticker: turn.sticker,
      image: turnImage(turn)
    })),
    items: [],
    searchText: `${[.../* @__PURE__ */ new Set([conversationTitle(record, character, language), record.title])].join(" ")} ${record.turns.map(turnText).join(" ")}`
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
