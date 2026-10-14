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

  // src/i18n/comic.js
  var rows = {
    toBeContinued: ["\u672A\u5B8C\u5F85\u7EED...", "To be continued...", "\u3064\u3065\u304F..."],
    plannerSwitchScheduler: ["\u70B9\u51FB\u8FD9\u91CC\uFF0C\u67E5\u770B\u5B9A\u65F6", "Click here to view reminders", "\u3053\u3053\u3092\u30AF\u30EA\u30C3\u30AF\u3057\u3066\u30EA\u30DE\u30A4\u30F3\u30C0\u30FC\u3078"],
    plannerSwitchTodo: ["\u70B9\u51FB\u8FD9\u91CC\uFF0C\u56DE\u5230\u5F85\u529E", "Click here to return to tasks", "\u3053\u3053\u3092\u30AF\u30EA\u30C3\u30AF\u3057\u3066\u30BF\u30B9\u30AF\u3078\u623B\u308B"],
    noteFullscreen: ["\u5168\u5C4F\u9605\u8BFB", "Read fullscreen", "\u5168\u753B\u9762\u3067\u8AAD\u3080"],
    noteExitFullscreen: ["\u9000\u51FA\u5168\u5C4F", "Exit fullscreen", "\u5168\u753B\u9762\u3092\u9589\u3058\u308B"],
    conversationTitle: ["\u4E0E{{names}}\u7684\u5BF9\u8BDD", "Conversation with {{names}}", "{{names}}\u3068\u306E\u4F1A\u8A71"],
    conversationYou: ["\u4F60", "you", "\u3042\u306A\u305F"],
    user: ["\u7528\u6237", "You", "\u30E6\u30FC\u30B6\u30FC"],
    everyone: ["\u5927\u5BB6", "Everyone", "\u307F\u3093\u306A"],
    pageFailed: ["\u8FD9\u4E00\u9875\u52A0\u8F7D\u5931\u8D25\u4E86", "This page failed to load", "\u3053\u306E\u30DA\u30FC\u30B8\u3092\u8AAD\u307F\u8FBC\u3081\u307E\u305B\u3093\u3067\u3057\u305F"],
    pageRetry: ["\u91CD\u8BD5", "Retry", "\u518D\u8A66\u884C"],
    noteTagsExample: ["\u7F8E\u98DF, \u65C5\u884C, \u653B\u7565", "Food, Travel, Guides", "\u30B0\u30EB\u30E1, \u65C5\u884C, \u30AC\u30A4\u30C9"],
    tableHeadersExample: ["\u57CE\u5E02	\u4EBA\u5747\u9884\u7B97", "City	Budget per person", "\u90FD\u5E02	\u4E00\u4EBA\u3042\u305F\u308A\u306E\u4E88\u7B97"],
    tableRowsExample: ["\u6210\u90FD	1200\n\u91CD\u5E86	800", "Chengdu	1200\nChongqing	800", "\u6210\u90FD	1200\n\u91CD\u6176	800"],
    chartBar: ["\u67F1\u72B6\u56FE", "Bar Chart", "\u68D2\u30B0\u30E9\u30D5"],
    chartLine: ["\u6298\u7EBF\u56FE", "Line Chart", "\u6298\u308C\u7DDA\u30B0\u30E9\u30D5"],
    chartPie: ["\u997C\u56FE", "Pie Chart", "\u5186\u30B0\u30E9\u30D5"],
    chartCategoriesExample: ["\u5B63\u5EA61\n\u5B63\u5EA62\n\u5B63\u5EA63", "Quarter 1\nQuarter 2\nQuarter 3", "\u7B2C1\u56DB\u534A\u671F\n\u7B2C2\u56DB\u534A\u671F\n\u7B2C3\u56DB\u534A\u671F"],
    chartSeriesExample: ["\u9500\u552E\u989D	120,180,240", "Sales	120,180,240", "\u58F2\u4E0A	120,180,240"],
    series: ["\u7CFB\u5217", "Series", "\u7CFB\u5217"],
    series1: ["\u7CFB\u5217 1", "Series 1", "\u7CFB\u5217 1"],
    categoryA: ["\u7C7B\u522B A", "Category A", "\u30AB\u30C6\u30B4\u30EA A"],
    categoryB: ["\u7C7B\u522B B", "Category B", "\u30AB\u30C6\u30B4\u30EA B"],
    mermaidExample: ["graph TD\n  A[\u5F00\u59CB] --> B[\u8FC7\u7A0B]\n  B --> C[\u7ED3\u675F]", "graph TD\n  A[Start] --> B[Process]\n  B --> C[End]", "graph TD\n  A[\u958B\u59CB] --> B[\u624B\u9806]\n  B --> C[\u7D42\u4E86]"],
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
    alt_ripple: ["\u6C34\u9762\u7559\u4E0B\u7684\u5929\u7A7A\u4E0E\u82B1\u74E3", "Sky and petals reflected on the water", "\u6C34\u9762\u306B\u6620\u308B\u7A7A\u3068\u82B1\u3073\u3089"],
    "alt_emotion-n": ["Nana \u4F1E\u4E0B\u6E29\u67D4\u7684\u4FA7\u8138", "Nana's gentle profile beneath an umbrella", "\u5098\u306E\u4E0B\u306E Nana \u306E\u512A\u3057\u3044\u6A2A\u9854"],
    "alt_emotion-v": ["Vivian \u8F7B\u629A\u80F8\u524D\uFF0C\u7559\u610F\u81EA\u5DF1\u7684\u5FC3\u60C5", "Vivian rests a hand on her chest, listening to her feelings", "\u80F8\u306B\u305D\u3063\u3068\u624B\u3092\u5F53\u3066\u3001\u81EA\u5206\u306E\u6C17\u6301\u3061\u3092\u611F\u3058\u308B Vivian"],
    "alt_cafe-exterior": ["\u96E8\u5929\u7684\u8857\u9053\u4E0A\uFF0C\u5496\u5561\u5385\u900F\u7740\u6696\u5149", "A caf\xE9 glows warmly on a rainy street", "\u96E8\u306E\u901A\u308A\u306B\u3001\u30AB\u30D5\u30A7\u306E\u6E29\u304B\u306A\u660E\u304B\u308A\u304C\u3053\u307C\u308C\u308B"],
    "alt_cafe-interior": ["Vivian \u548C Nana \u5750\u5728\u5496\u5561\u5385\u7A97\u8FB9\uFF0C\u5206\u4EAB\u684C\u4E0A\u7684\u65E5\u8BB0", "Vivian and Nana share a journal beside the caf\xE9 window", "\u30AB\u30D5\u30A7\u306E\u7A93\u8FBA\u3067\u3001\u65E5\u8A18\u3092\u4E00\u7DD2\u306B\u8AAD\u3080 Vivian \u3068 Nana"],
    alt_hands: ["\u4E24\u53EA\u624B\u5728\u540C\u4E00\u628A\u4F1E\u67C4\u4E0A\u9760\u8FD1", "Two hands draw closer on the same umbrella handle", "\u540C\u3058\u5098\u306E\u6301\u3061\u624B\u3067\u3001\u3075\u305F\u3064\u306E\u624B\u304C\u8FD1\u3065\u304F"],
    alt_together: ["Vivian \u4E0E Nana \u5728\u96E8\u4E2D\u5171\u6491\u4E00\u628A\u4F1E", "Vivian and Nana share an umbrella in the rain", "\u96E8\u306E\u4E2D\u3067\u4E00\u672C\u306E\u5098\u3092\u5206\u3051\u5408\u3046 Vivian \u3068 Nana"],
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
    title1: ["\u5728\u96E8\u91CC\u627E\u5230\u4F60", "Finding You in the Rain", "\u96E8\u306E\u4E2D\u3067\u541B\u3092\u898B\u3064\u3051\u3066"],
    title2: ["\u5FC3\u60C5\u7684\u989C\u8272", "Colors of the Heart", "\u5FC3\u306E\u8272"],
    title3: ["\u628A\u4ECA\u5929\u7559\u4E0B", "Keeping Today", "\u4ECA\u65E5\u3092\u6B8B\u3057\u3066"],
    title4: ["\u4E00\u70B9\u70B9\u9760\u8FD1", "A Little Closer", "\u5C11\u3057\u305A\u3064\u8FD1\u3065\u3044\u3066"],
    title5: ["\u7EA6\u5B9A", "Promises", "\u7D04\u675F"],
    title6: ["\u5B57\u91CC\u884C\u95F4", "Between the Lines", "\u884C\u9593\u306B"],
    title7: ["\u7559\u4E0B\u7684\u8DB3\u8FF9", "Footprints We Leave", "\u6B8B\u3057\u305F\u8DB3\u8DE1"],
    title8: ["\u7535\u8F66\u91CC", "On the Train", "\u96FB\u8ECA\u306E\u4E2D\u3067"],
    title9: ["\u6E05\u6668", "Morning", "\u671D"],
    contents: ["CONTENT", "CONTENT", "CONTENT"],
    vivianThoughtLabel: ["Vivian \u5F53\u524D\u60F3\u6CD5", "Vivian\u2019s current thought", "Vivian \u306E\u4ECA\u306E\u60F3\u3044"],
    nanaThoughtLabel: ["Nana \u5F53\u524D\u60F3\u6CD5", "Nana\u2019s current thought", "Nana \u306E\u4ECA\u306E\u60F3\u3044"],
    browse: ["\u968F\u624B\u7FFB\u9605", "Browse", "\u6C17\u307E\u307E\u306B\u3081\u304F\u308B"],
    chapters: ["\u6545\u4E8B\u7AE0\u8282", "Story Chapters", "\u7269\u8A9E\u306E\u7AE0"],
    destinations: ["\u5185\u5BB9\u5B9A\u4F4D", "Jump to Content", "\u5185\u5BB9\u3078\u79FB\u52D5"],
    previous: ["\u4E0A\u4E00\u9875", "Previous Page", "\u524D\u306E\u30DA\u30FC\u30B8"],
    next: ["\u4E0B\u4E00\u9875", "Next Page", "\u6B21\u306E\u30DA\u30FC\u30B8"],
    pageLabel: ["{{title}}\uFF0C\u7B2C {{page}} \u9875\uFF0C\u5171 {{total}} \u9875", "{{title}}, page {{page}} of {{total}}", "{{title}}\u3001{{total}} \u30DA\u30FC\u30B8\u4E2D {{page}} \u30DA\u30FC\u30B8"],
    gesture: ["{{title}}\uFF0C\u5DE6\u53F3\u62D6\u62FD\u6216\u6EDA\u8F6E\u7FFB\u9875", "{{title}}. Drag sideways or scroll to turn pages.", "{{title}}\u3002\u5DE6\u53F3\u306B\u30C9\u30E9\u30C3\u30B0\u3001\u307E\u305F\u306F\u30B9\u30AF\u30ED\u30FC\u30EB\u3067\u30DA\u30FC\u30B8\u3092\u3081\u304F\u308A\u307E\u3059\u3002"],
    refresh: ["\u5237\u65B0\u8BB0\u5FC6", "Refresh Memories", "\u8A18\u61B6\u3092\u66F4\u65B0"],
    expand: ["\u5C55\u5F00{{name}}", "Open {{name}}", "{{name}}\u3092\u958B\u304F"],
    collapse: ["\u6536\u56DE{{name}}", "Close {{name}}", "{{name}}\u3092\u9589\u3058\u308B"],
    panelMemory: ["\u8BB0\u5FC6\u4E0E\u5BF9\u8BDD", "Memories & Conversations", "\u8A18\u61B6\u3068\u4F1A\u8A71"],
    panelDiary: ["\u4ECA\u5929\u7684\u624B\u8BB0", "Today's Journal", "\u4ECA\u65E5\u306E\u65E5\u8A18"],
    panelNotebook: ["\u7559\u4E0B\u7684\u8DB3\u8FF9", "Footprints We Leave", "\u6B8B\u3057\u305F\u8DB3\u8DE1"],
    panelPlanner: ["\u5C0F\u5C0F\u7684\u7EA6\u5B9A", "Little Promises", "\u5C0F\u3055\u306A\u7D04\u675F"],
    readingMemory: ["\u5171\u540C\u8BB0\u5FC6 / MEMORY", "Shared Memories / MEMORY", "\u3075\u305F\u308A\u306E\u8A18\u61B6 / MEMORY"],
    readingDiary: ["\u65E5\u5E38\u624B\u8BB0 / JOURNAL", "Daily Journal / JOURNAL", "\u65E5\u3005\u306E\u65E5\u8A18 / JOURNAL"],
    readingNotebook: ["\u5C0F\u5C0F\u7075\u611F / NOTES", "Little Inspirations / NOTES", "\u5C0F\u3055\u306A\u3072\u3089\u3081\u304D / NOTES"],
    readingPlanner: ["\u6162\u6162\u5B9E\u73B0 / PLANS", "One Step at a Time / PLANS", "\u5C11\u3057\u305A\u3064\u53F6\u3048\u3066 / PLANS"],
    rainArrival: ["\u7535\u8F66\u628A\u8FDC\u65B9\u7559\u5728\u8EAB\u540E\uFF0C\u96E8\u58F0\u6F2B\u8FC7\u7AD9\u53F0\u3002", "The train leaves the distance behind. Rain washes over the platform.", "\u96FB\u8ECA\u306F\u9060\u304F\u3092\u5F8C\u306B\u3057\u3066\u3001\u96E8\u97F3\u304C\u30DB\u30FC\u30E0\u3092\u6E80\u305F\u3059\u3002"],
    rainPoem: ["\u65E0\u4EBA\u6491\u8D77\u7684\u6674\u7A7A\uFF0C\u5316\u4F5C\u5979\u88D9\u89D2\u7EC6\u788E\u7684\u96E8\u3002", "With no umbrella to hold up the sky, rain gathers at the hem of her skirt.", "\u8AB0\u3082\u5DEE\u3057\u304B\u3051\u306A\u3044\u6674\u308C\u7A7A\u304C\u3001\u5F7C\u5973\u306E\u88FE\u306B\u7D30\u304B\u306A\u96E8\u3068\u306A\u308B\u3002"],
    feelings: ["\u5FC3\u60C5\uFF0C\n\u6709\u4E86\u989C\u8272\u3002", "Feelings,\nnow in color.", "\u5FC3\u306B\u3001\n\u8272\u304C\u3064\u3044\u305F\u3002"],
    diaryCaption: ["\u4E00\u4EF6\u5C0F\u4E8B\uFF0C\u4E00\u70B9\u5FC3\u60C5\u3002\n\u4ECA\u5929\uFF0C\u4E5F\u503C\u5F97\u88AB\u8BB0\u5F97\u3002", "A little moment, a little feeling.\nToday is worth remembering, too.", "\u5C0F\u3055\u306A\u51FA\u6765\u4E8B\u3001\u5C0F\u3055\u306A\u6C17\u6301\u3061\u3002\n\u4ECA\u65E5\u3082\u3001\u899A\u3048\u3066\u304A\u304D\u305F\u3044\u4E00\u65E5\u3002"],
    vivianGoals: ["Vivian \xB7 \u60F3\u505A\u7684\u4E8B", "Vivian \xB7 Things to Do", "Vivian \xB7 \u3084\u308A\u305F\u3044\u3053\u3068"],
    nanaGoals: ["Nana \xB7 \u60F3\u505A\u7684\u4E8B", "Nana \xB7 Things to Do", "Nana \xB7 \u3084\u308A\u305F\u3044\u3053\u3068"],
    plansCaption: ["\u8FD8\u6709\u60F3\u505A\u7684\u4E8B\u3002", "Still Things to Do.", "\u307E\u3060\u3001\u3084\u308A\u305F\u3044\u3053\u3068\u3002"],
    morning: ["\u65E9\u6668", "Morning", "\u671D"],
    promiseCaption: ["\u628A\u7EA6\u5B9A\u5E26\u5728\u8EAB\u8FB9\u3002", "Keep our promises close.", "\u7D04\u675F\u3092\u3001\u305D\u3070\u306B\u3002"],
    unfinished: ["\u672A\u5B8C\u7684\n\u5FC3\u4E8B\u3002", "Thoughts\nstill unfinished.", "\u307E\u3060\u4F1D\u3048\u304D\u308C\u306A\u3044\n\u60F3\u3044\u3002"],
    everydayKicker: ["\u6211\u4EEC\u7684\u65E5\u5E38 / 01", "OUR EVERYDAY / 01", "\u3075\u305F\u308A\u306E\u65E5\u3005 / 01"],
    feelingsKicker: ["\u5FC3\u60C5 / 03", "FEELINGS / 03", "\u6C17\u6301\u3061 / 03"],
    plansKicker: ["\u5C0F\u5C0F\u8BA1\u5212 / 06", "LITTLE PLANS / 06", "\u5C0F\u3055\u306A\u4E88\u5B9A / 06"],
    unfinishedKicker: ["\u672A\u5B8C\u5F85\u7EED / 08", "TO BE CONTINUED / 08", "\u3064\u3065\u304F / 08"],
    alarm: ["\u53EE\u94C3\u94C3\u2026", "Ring, ring\u2026", "\u30B8\u30EA\u30EA\u2026"],
    heartbeat: ["\u6026\u2026", "Thump\u2026", "\u30C9\u30AD\u2026"],
    trainSound: ["\u5494 \u54D2", "Click-clack", "\u30AC\u30BF\u30F3 \u30B4\u30C8\u30F3"],
    loadingThought: ["\u6B63\u5728\u503E\u542C\u5FC3\u58F0\u2026", "Listening to inner thoughts\u2026", "\u5FC3\u306E\u58F0\u306B\u8033\u3092\u6F84\u307E\u305B\u3066\u3044\u307E\u3059\u2026"],
    failedThought: ["\u5FC3\u58F0\u6682\u65F6\u65E0\u6CD5\u8BFB\u53D6", "Inner thoughts are unavailable right now", "\u4ECA\u306F\u5FC3\u306E\u58F0\u3092\u8AAD\u307F\u53D6\u308C\u307E\u305B\u3093"],
    disabledThought: ["\u5185\u5FC3\u72EC\u767D\u5DF2\u5173\u95ED", "Inner monologue is turned off", "\u5185\u5FC3\u306E\u72EC\u767D\u306F\u30AA\u30D5\u3067\u3059"],
    quietThought: ["\u6B64\u523B\u5B89\u9759\u7740\u3002", "Quiet for a moment.", "\u4ECA\u306F\u9759\u304B\u306B\u3002"],
    failedGoal: ["\u76EE\u6807\u6682\u65F6\u65E0\u6CD5\u8BFB\u53D6", "Goals are unavailable right now", "\u4ECA\u306F\u76EE\u6A19\u3092\u8AAD\u307F\u53D6\u308C\u307E\u305B\u3093"],
    quietGoal: ["\u4ECA\u5929\u4E0D\u8D76\u8DEF\uFF0C\u966A\u4F60\u628A\u65E5\u5B50\u6162\u6162\u8FC7\u3002", "No rush today. Let's take our time together.", "\u4ECA\u65E5\u306F\u6025\u304C\u305A\u3001\u541B\u3068\u3086\u3063\u304F\u308A\u904E\u3054\u3057\u305F\u3044\u3002"],
    loadingMood: ["\u8BFB\u53D6\u5FC3\u60C5\u4E2D\u2026", "Reading the mood\u2026", "\u6C17\u5206\u3092\u8AAD\u307F\u53D6\u3063\u3066\u3044\u307E\u3059\u2026"],
    failedMood: ["\u5FC3\u60C5\u6682\u65F6\u65E0\u6CD5\u8BFB\u53D6", "Mood is unavailable right now", "\u4ECA\u306F\u6C17\u5206\u3092\u8AAD\u307F\u53D6\u308C\u307E\u305B\u3093"],
    noMood: ["\u6682\u65E0\u5FC3\u60C5", "No mood available", "\u6C17\u5206\u306F\u307E\u3060\u4E0D\u660E\u3067\u3059"],
    online: ["\u5728\u7EBF", "Online", "\u30AA\u30F3\u30E9\u30A4\u30F3"],
    busy: ["\u5FD9\u788C", "Busy", "\u53D6\u308A\u8FBC\u307F\u4E2D"],
    rest: ["\u4F11\u606F", "Resting", "\u4F11\u61A9\u4E2D"],
    offline: ["\u79BB\u7EBF", "Offline", "\u30AA\u30D5\u30E9\u30A4\u30F3"],
    unknown: ["\u72B6\u6001\u672A\u77E5", "Status unknown", "\u72B6\u614B\u4E0D\u660E"],
    seconds: ["{{n}}\u79D2", "{{n}}s", "{{n}}\u79D2"],
    minutes: ["{{n}}\u5206\u949F", "{{n}}m", "{{n}}\u5206"],
    hours: ["{{n}}\u5C0F\u65F6", "{{n}}h", "{{n}}\u6642\u9593"],
    conversationViewport: ["\u6700\u65B0\u5BF9\u8BDD\uFF0C\u81EA\u52A8\u7F13\u6162\u4E0B\u6ED1", "Latest conversation, scrolling slowly", "\u6700\u65B0\u306E\u4F1A\u8A71\u3001\u3086\u3063\u304F\u308A\u81EA\u52D5\u30B9\u30AF\u30ED\u30FC\u30EB"],
    conversationLog: ["\u6700\u65B0\u4F1A\u8BDD\u539F\u59CB\u6D88\u606F", "Original messages from the latest conversation", "\u6700\u65B0\u306E\u4F1A\u8A71\u306E\u5143\u306E\u30E1\u30C3\u30BB\u30FC\u30B8"],
    audience: ["\u5BF9 {{name}} \u8BF4", "Speaking to {{name}}", "{{name}}\u306B\u8A71\u3057\u304B\u3051\u3066\u3044\u307E\u3059"],
    conversationLoading: ["\u6B63\u5728\u8BFB\u53D6\u5BF9\u8BDD\u2026", "Loading the conversation\u2026", "\u4F1A\u8A71\u3092\u8AAD\u307F\u8FBC\u3093\u3067\u3044\u307E\u3059\u2026"],
    conversationEmpty: ["\u7B49\u4E0B\u4E00\u6B21\u4EA4\u6D41\uFF0C\u6545\u4E8B\u4F1A\u4ECE\u8FD9\u91CC\u63A5\u7740\u5199\u3002", "Our story will continue here with the next conversation.", "\u6B21\u306B\u8A71\u3059\u3068\u304D\u3001\u7269\u8A9E\u306F\u3053\u3053\u304B\u3089\u7D9A\u3044\u3066\u3044\u304F\u3002"],
    loadingVivian: ["\u7B49\u4E00\u4E0B\uFF0C\u6211\u8FD8\u5728\u7406\u6E05\u5FC3\u91CC\u7684\u8BDD\u3002", "One moment. I'm still sorting out my thoughts.", "\u5C11\u3057\u5F85\u3063\u3066\u306D\u3002\u307E\u3060\u5FC3\u306E\u8A00\u8449\u3092\u6574\u7406\u3057\u3066\u3044\u308B\u306E\u3002"],
    loadingNana: ["\u7A0D\u7B49\u7247\u523B\uFF0C\u8BA9\u6211\u542C\u6E05\u81EA\u5DF1\u7684\u5FC3\u7EEA\u3002", "A moment, please. Let me listen to my own feelings.", "\u5C11\u3057\u5F85\u3063\u3066\u3002\u81EA\u5206\u306E\u6C17\u6301\u3061\u3092\u78BA\u304B\u3081\u305F\u3044\u306E\u3002"],
    errorVivian: ["\u8FD9\u4F1A\u513F\u6709\u4E9B\u8BDD\u6CA1\u63A5\u4E0A\uFF0C\u7B49\u4E00\u4E0B\u518D\u542C\u6211\u8BF4\u3002", "My words aren't coming through right now. Listen again in a little while.", "\u4ECA\u306F\u3046\u307E\u304F\u8A00\u8449\u304C\u5C4A\u304B\u306A\u3044\u306E\u3002\u5C11\u3057\u3057\u3066\u304B\u3089\u3001\u307E\u305F\u805E\u3044\u3066\u306D\u3002"],
    errorNana: ["\u8FD9\u4F1A\u513F\u6682\u65F6\u6CA1\u8BFB\u5230\u5FC3\u7EEA\uFF0C\u6211\u4EEC\u7A0D\u540E\u518D\u63A5\u7740\u770B\u3002", "I can't read my feelings right now. Let's return to them later.", "\u4ECA\u306F\u6C17\u6301\u3061\u304C\u8AAD\u307F\u53D6\u308C\u306A\u3044\u307F\u305F\u3044\u3002\u3042\u3068\u3067\u7D9A\u304D\u3092\u898B\u307E\u3057\u3087\u3046\u3002"],
    moodMissingVivian: ["\u5FC3\u60C5\u8FD8\u6CA1\u770B\u6E05\uFF0C\u5148\u7559\u4E00\u5C0F\u683C\u7A7A\u767D\u3002", "I haven't found my mood yet. Let's leave a little space.", "\u307E\u3060\u6C17\u5206\u304C\u306F\u3063\u304D\u308A\u3057\u306A\u3044\u306E\u3002\u5C0F\u3055\u306A\u7A7A\u767D\u3092\u6B8B\u3057\u3066\u304A\u3053\u3046\u3002"],
    moodMissingNana: ["\u5FC3\u7EEA\u6682\u65F6\u672A\u660E\uFF0C\u7559\u4E00\u70B9\u5B89\u9759\u4E5F\u597D\u3002", "My feelings are unclear for now. A little quiet is fine.", "\u307E\u3060\u6C17\u6301\u3061\u306F\u898B\u3048\u306A\u3044\u3051\u308C\u3069\u3001\u5C11\u3057\u9759\u304B\u306B\u3057\u3066\u3044\u3066\u3082\u3044\u3044\u306D\u3002"],
    missingVivian: ["\u8FD9\u4E00\u683C\u7684\u5FC3\u4E8B\uFF0C\u8FD8\u6CA1\u542C\u6E05\u5462\u3002", "I haven't quite heard the thought in this panel yet.", "\u3053\u306E\u30B3\u30DE\u306E\u60F3\u3044\u306F\u3001\u307E\u3060\u306F\u3063\u304D\u308A\u805E\u3053\u3048\u306A\u3044\u306E\u3002"],
    missingNana: ["\u8FD9\u4EFD\u5FC3\u7EEA\u6682\u65F6\u672A\u660E\uFF0C\u4E0D\u5FC5\u6025\u7740\u66FF\u5B83\u4E0B\u7ED3\u8BBA\u3002", "These feelings aren't clear yet. There's no need to decide what they mean.", "\u3053\u306E\u6C17\u6301\u3061\u306F\u307E\u3060\u898B\u3048\u306A\u3044\u304B\u3089\u3001\u6025\u3044\u3067\u7B54\u3048\u3092\u6C7A\u3081\u306A\u304F\u3066\u3082\u3044\u3044\u3002"]
  };
  var comicTranslations = Object.fromEntries(["zh-CN", "en", "ja"].map((lang, index) => [lang, Object.fromEntries(Object.entries(rows).map(([key, values]) => [key, values[index]]))]));
  function comicLocale(language = "zh-CN") {
    return language.startsWith("ja") ? "ja" : language.startsWith("en") ? "en" : "zh-CN";
  }
  function comicText(key, language = "zh-CN") {
    return comicTranslations[comicLocale(language)][key] ?? key;
  }
  var literalKeys = new Map(Object.entries(comicTranslations["zh-CN"]).map(([key, value]) => [value, key]));

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
  function conversationTitle(record, character, language = "zh-CN") {
    const others = record.participants.filter((name) => name !== character);
    const originalNames = others.map((name) => name === "user" ? "\u4F60" : name === "vivian" ? "Vivian" : name === "nana" ? "Nana" : name);
    if (record.title !== `\u4E0E${originalNames.join("\u3001")}\u7684\u5BF9\u8BDD`) return record.title;
    const names = others.map((name) => name === "user" ? comicText("conversationYou", language) : name === "vivian" ? "Vivian" : name === "nana" ? "Nana" : name);
    return comicText("conversationTitle", language).replace("{{names}}", names.join(comicLocale(language) === "en" ? ", " : "\u3001"));
  }
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
      text: turn.text,
      timestamp: turn.timestamp,
      sticker: turn.sticker
    })),
    items: [],
    searchText: `${[.../* @__PURE__ */ new Set([conversationTitle(record, character, language), record.title])].join(" ")} ${record.turns.map((turn) => turn.text).join(" ")}`
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
