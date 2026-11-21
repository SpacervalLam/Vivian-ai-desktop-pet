import React, { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Block, NoteBook } from './notebook-types';
import { compilePageSource, PageSourceError, pageEnter, sourceFromBlocks } from './notebookPageSource';
import './NotebookPageEditor.css';

type ElementTemplate = { id: string; label: string; hint: string; source: string };
function templates(zh: boolean): ElementTemplate[] {
  const text = zh ? '在这里写内容' : 'Write here';
  const title = zh ? '小标题' : 'Heading';
  return [
    { id: 'card', label: zh ? '卡片' : 'Card', hint: '<section>', source: `:::card title="${title}"\n${text}\n:::` },
    { id: 'row', label: zh ? '分栏' : 'Columns', hint: 'grid + <div>', source: `:::row columns=2 gap=20\n:::column\n${zh ? '左侧内容' : 'Left column'}\n:::\n:::column\n${zh ? '右侧内容' : 'Right column'}\n:::\n:::` },
    { id: 'callout', label: zh ? '提示框' : 'Callout', hint: '<aside>', source: `:::callout background="#f4eee6" padding=16 radius=12\n${text}\n:::` },
    { id: 'section', label: zh ? '章节' : 'Section', hint: '<section>', source: `:::section title="${title}"\n${text}\n:::` },
    { id: 'text', label: zh ? '文字样式' : 'Styled text', hint: '<div style>', source: `:::text color="#8c637d" size=18 align=center\n${text}\n:::` },
    { id: 'image', label: zh ? '图片' : 'Image', hint: '<figure> + <img>', source: `:::image src="https://example.com/image.jpg" caption="${zh ? '图片说明' : 'Caption'}"\n:::` },
    { id: 'table', label: zh ? '表格' : 'Table', hint: '<table>', source: zh ? '| 项目 | 内容 |\n| --- | --- |\n| 示例 | 内容 |' : '| Item | Content |\n| --- | --- |\n| Example | Content |' },
    { id: 'quote', label: zh ? '引用' : 'Quote', hint: '<blockquote>', source: `:::quote author="${zh ? '作者' : 'Author'}"\n${text}\n:::` },
    { id: 'code', label: zh ? '代码' : 'Code', hint: '<pre> + <code>', source: ':::code lang="text"\nconsole.log("Hello");\n:::' },
    { id: 'chart', label: zh ? '图表' : 'Chart', hint: zh ? '编辑数据' : 'Edit data', source: `:::chart\n${JSON.stringify({ chart_type: 'bar', title: zh ? '数据对比' : 'Comparison', categories: ['A', 'B'], series: [{ name: zh ? '数值' : 'Value', data: [10, 20] }] }, null, 2)}\n:::` },
    { id: 'mermaid', label: zh ? '流程图' : 'Diagram', hint: 'Mermaid', source: ':::mermaid\ngraph TD\n  A[Start] --> B[End]\n:::' },
    { id: 'html', label: 'HTML', hint: zh ? '原始 HTML' : 'Raw HTML', source: `:::html\n<div class="card">${text}</div>\n:::` },
  ];
}

export const NotebookPageEditor: React.FC<{
  note: NoteBook; lang: string; saving: boolean; onChange: (blocks: Block[] | null) => void;
  onSave: () => void; onValidityChange: (invalid: boolean) => void;
}> = ({ note, lang, saving, onChange, onSave, onValidityChange }) => {
  const [source, setSource] = useState(() => sourceFromBlocks(note.blocks));
  const [preview, setPreview] = useState(true);
  const [help, setHelp] = useState(false);
  const [html, setHtml] = useState('');
  const [previewError, setPreviewError] = useState('');
  const [previewPending, setPreviewPending] = useState(true);
  const [retry, setRetry] = useState(0);
  const [caret, setCaret] = useState(0);
  const [activeCommand, setActiveCommand] = useState(0);
  const ref = useRef<HTMLTextAreaElement>(null);
  // Invisible acquisition metadata is kept out of the writing surface.
  const metadata = useRef(note.blocks.filter(b => b.type === 'meta'));
  const zh = lang.startsWith('zh');
  const items = useMemo(() => templates(zh), [zh]);
  const result = useMemo(() => {
    try { return { blocks: compilePageSource(source), error: null }; }
    catch (e) { return { blocks: null, error: e instanceof PageSourceError ? e : new PageSourceError(1, String(e)) }; }
  }, [source]);
  const errorParts = result.error?.message.split(' / ');
  const errorText = errorParts ? (zh ? errorParts[0] + (errorParts[1]?.match(/: (.+)$/)?.[1] ? `：${errorParts[1].match(/: (.+)$/)![1]}` : '') : errorParts[1] ?? errorParts[0]) : '';
  const preceding = source.slice(0, caret);
  const lineStart = preceding.lastIndexOf('\n') + 1;
  const command = caret >= 0 ? /^\/([\w\u4e00-\u9fff-]*)$/.exec(preceding.slice(lineStart)) : null;
  const matches = command ? items.filter(item => item.id.includes(command[1].toLowerCase()) || item.label.includes(command[1])) : [];

  useEffect(() => { onValidityChange(Boolean(result.error)); }, [result.error, onValidityChange]);
  useEffect(() => { setActiveCommand(0); }, [command?.[1]]);
  useEffect(() => {
    if (!preview || result.error) return;
    let disposed = false;
    setPreviewPending(true);
    const timer = window.setTimeout(() => {
      void invoke<string>('render_notebook_preview', { note }).then(next => {
        if (!disposed) { setHtml(next); setPreviewError(''); }
      }).catch(e => {
        if (!disposed) setPreviewError(String(e));
      }).finally(() => { if (!disposed) setPreviewPending(false); });
    }, 250);
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [note, preview, retry, result.error]);

  const change = (next: string) => {
    setSource(next);
    try {
      // The backend sanitizes raw HTML; the preview iframe has no script or origin privileges.
      const blocks = compilePageSource(next);
      onValidityChange(false);
      onChange([...blocks, ...metadata.current]);
    } catch {
      onValidityChange(true);
      // Mark invalid typing dirty without replacing the last valid document.
      onChange(null);
    }
  };
  const replace = (start: number, end: number, value: string, selectionStart = start + value.length, selectionEnd = selectionStart) => {
    const el = ref.current;
    if (!el) return;
    el.focus(); el.setSelectionRange(start, end);
    if (!document.execCommand('insertText', false, value)) el.setRangeText(value, start, end, 'end');
    change(el.value);
    el.setSelectionRange(selectionStart, selectionEnd);
    setCaret(selectionStart);
  };
  const wrap = (before: string, after = '', block = false) => {
    const el = ref.current;
    if (!el) return;
    const start = block && el.selectionStart > 0 ? source.lastIndexOf('\n', el.selectionStart - 1) + 1 : el.selectionStart;
    const end = el.selectionEnd;
    const selected = source.slice(start, end) || (zh ? '文字' : 'Text');
    const value = block ? selected.split('\n').map(line => before + line).join('\n') : before + selected + after;
    replace(start, end, value, start + before.length, block ? start + value.length : start + before.length + selected.length);
  };
  const insert = (item: ElementTemplate) => {
    const el = ref.current;
    if (!el) return;
    const start = command ? lineStart : el.selectionStart;
    const end = command ? caret : el.selectionEnd;
    const selected = source.slice(start, end);
    const snippet = selected && !command ? item.source.replace(zh ? '在这里写内容' : 'Write here', selected) : item.source;
    const prefix = start > 0 && source[start - 1] !== '\n' ? '\n\n' : '';
    const suffix = end < source.length && source[end] !== '\n' ? '\n\n' : '\n';
    const placeholder = snippet.indexOf(zh ? '在这里写内容' : 'Write here');
    const selectionStart = start + prefix.length + (placeholder >= 0 ? placeholder : snippet.length);
    replace(start, end, prefix + snippet + suffix, selectionStart, placeholder >= 0 ? selectionStart + (zh ? 6 : 10) : selectionStart);
  };
  const goToError = () => {
    if (!result.error || !ref.current) return;
    const offset = source.split('\n').slice(0, result.error.line - 1).reduce((sum, line) => sum + line.length + 1, 0);
    ref.current.focus(); ref.current.setSelectionRange(offset, offset); setCaret(offset);
  };
  return <div className="note-page-editor">
    <div className="note-page-toolbar" role="toolbar" aria-label={zh ? '页面元素' : 'Page elements'}>
      <button type="button" className="rec-btn is-ghost" onMouseDown={e => e.preventDefault()} onClick={() => wrap('## ', '', true)}>{zh ? '标题' : 'Heading'}</button>
      <button type="button" className="rec-btn is-ghost" onMouseDown={e => e.preventDefault()} onClick={() => wrap('**', '**')}>{zh ? '粗体' : 'Bold'}</button>
      <button type="button" className="rec-btn is-ghost" onMouseDown={e => e.preventDefault()} onClick={() => wrap('- ', '', true)}>{zh ? '列表' : 'List'}</button>
      <button type="button" className="rec-btn is-ghost" onMouseDown={e => e.preventDefault()} onClick={() => wrap('- [ ] ', '', true)}>{zh ? '待办' : 'Task'}</button>
      <button type="button" className="rec-btn is-ghost" onMouseDown={e => e.preventDefault()} onClick={() => wrap('[', '](https://)')}>{zh ? '链接' : 'Link'}</button>
      {items.filter(item => ['card', 'row', 'callout', 'image', 'table'].includes(item.id)).map(item => <button key={item.id} type="button" className="rec-btn is-ghost" onMouseDown={e => e.preventDefault()} onClick={() => insert(item)}>{item.label}</button>)}
      <details className="note-page-insert"><summary className="rec-btn">{zh ? '更多元素' : 'More elements'}</summary><div>
        {items.filter(item => !['card', 'row', 'callout', 'image', 'table'].includes(item.id)).map(item => <button key={item.id} type="button" className="rec-btn is-ghost" onMouseDown={e => e.preventDefault()} onClick={e => { insert(item); e.currentTarget.closest('details')?.removeAttribute('open'); }}>{item.label}</button>)}
      </div></details>
      <button type="button" className="rec-btn" aria-pressed={preview} onClick={() => setPreview(v => !v)}>{preview ? (zh ? '收起预览' : 'Hide preview') : (zh ? '显示预览' : 'Show preview')}</button>
      <button type="button" className="rec-btn is-ghost" aria-expanded={help} onClick={() => setHelp(v => !v)}>{zh ? '写作提示' : 'Writing guide'}</button>
    </div>
    {help && <div className="note-page-guide">
      <p>{zh ? '直接写正文，使用工具栏或在新行输入 / 插入页面元素。::: 开始和结束一个容器，容器可以嵌套；改 color、padding、columns 等属性即可调整样式。' : 'Write directly, or use the toolbar or / on a new line to insert elements. ::: opens and closes a wrapper. Nest wrappers and adjust color, padding or columns.'}</p>
      <pre>{':::card title="Title" padding=20\n## Heading\n**Emphasis**\n:::row columns=2\n:::column\nLeft\n:::\n:::column\nRight\n:::\n:::\n:::'}</pre>
      <p>{zh ? '可用样式：color、background、size、align、padding、radius、width、gap。需要完整 HTML 时，插入 HTML 元素并在里面写标签。图表和流程图放在容器外。' : 'Styles: color, background, size, align, padding, radius, width, gap. Use an HTML wrapper for native tags. Place charts and diagrams outside wrappers.'}</p>
    </div>}
    {result.error && <div className="note-page-error" role="alert"><span>{zh ? `第 ${result.error.line} 行：` : `Line ${result.error.line}: `}{errorText}</span><button type="button" className="rec-btn" onClick={goToError}>{zh ? '定位' : 'Go to line'}</button></div>}
    <div className={`note-page-panes${preview ? ' with-preview' : ''}`}>
      <div className="note-page-writing">
        <div className="note-page-pane-label">{zh ? '页面正文' : 'Page content'} <span>{zh ? '输入 / 插入元素' : 'Type / to insert'}</span></div>
        <textarea ref={ref} className="note-page-source" aria-label={zh ? '页面正文' : 'Page content'} value={source} spellCheck={false}
          placeholder={zh ? '从这里开始写…\n\n## 小标题\n写下想保留的内容。\n\n输入 / 添加卡片、分栏、图片…' : 'Start writing…\n\n## Heading\nWrite what matters.\n\nType / for cards, columns, images…'}
          onChange={e => { change(e.target.value); setCaret(e.target.selectionStart); }} onSelect={e => setCaret(e.currentTarget.selectionStart)}
          onKeyDown={e => {
            if (e.nativeEvent.isComposing || e.keyCode === 229) return;
            if (matches.length && ['ArrowDown', 'ArrowUp', 'Enter', 'Escape'].includes(e.key)) {
              e.preventDefault();
              if (e.key === 'Enter') insert(matches[activeCommand % matches.length]);
              else if (e.key === 'Escape') setCaret(-1);
              else setActiveCommand(i => (i + (e.key === 'ArrowDown' ? 1 : matches.length - 1)) % matches.length);
              return;
            }
            if ((e.ctrlKey || e.metaKey) && ['s', 'b', 'i'].includes(e.key.toLowerCase())) {
              e.preventDefault();
              if (e.key.toLowerCase() === 's') { if (!result.error && !saving) onSave(); }
              else wrap(e.key.toLowerCase() === 'b' ? '**' : '*', e.key.toLowerCase() === 'b' ? '**' : '*');
            } else if (e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
              const edit = pageEnter(source, e.currentTarget.selectionStart, e.currentTarget.selectionEnd);
              if (edit.text !== '\n' || edit.start !== e.currentTarget.selectionStart) { e.preventDefault(); replace(edit.start, edit.end, edit.text, edit.caret); }
            } else if (e.key === 'Tab') {
              e.preventDefault();
              const el = e.currentTarget;
              const start = el.selectionStart === 0 ? 0 : source.lastIndexOf('\n', el.selectionStart - 1) + 1;
              if (e.shiftKey) {
                const spaces = /^ {1,2}/.exec(source.slice(start))?.[0].length ?? 0;
                if (spaces) replace(start, start + spaces, '', Math.max(start, el.selectionStart - spaces));
              } else replace(el.selectionStart, el.selectionEnd, '  ');
            }
          }} />
        {matches.length > 0 && caret >= 0 && <div className="note-page-commands" role="listbox" aria-label={zh ? '插入元素' : 'Insert element'}>
          {matches.map((item, i) => <button key={item.id} type="button" role="option" aria-selected={i === activeCommand} onMouseDown={e => e.preventDefault()} onClick={() => insert(item)}><span>{item.label}</span><small>{item.hint}</small></button>)}
        </div>}
      </div>
      {preview && <div className="note-page-preview">
        <div className="note-page-pane-label">{zh ? '页面预览' : 'Page preview'} <span role="status">{result.error ? (zh ? '显示上一次有效内容' : 'Last valid content') : previewPending ? (zh ? '更新中…' : 'Updating…') : (zh ? '与保存后的页面一致' : 'Matches the saved page')}</span></div>
        {previewError ? <div className="note-page-preview-error"><p>{zh ? '页面预览暂时不可用，正文仍可编辑。' : 'Preview is unavailable. You can continue writing.'}</p><button className="rec-btn" type="button" onClick={() => setRetry(i => i + 1)}>{zh ? '重试预览' : 'Retry preview'}</button></div>
          : <iframe title={zh ? '页面预览' : 'Page preview'} srcDoc={html} sandbox="" />}
      </div>}
    </div>
    <div className="note-page-hint">{zh ? 'Enter 续写列表 · Tab 缩进 / Shift+Tab 取消缩进 · Ctrl/⌘ B 加粗、I 斜体、S 保存' : 'Enter continues lists · Tab / Shift+Tab to indent · Ctrl/⌘ B bold, I italic, S save'}</div>
  </div>;
};
