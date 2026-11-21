/**
 * OfficePreview — 预览页里 Office 文档的渲染。
 *
 * 分流规则（由后端 `coding_read_file` 标成 kind === 'office' 后进到这里）：
 * - Word（.docx/.docm/.dotx/.dotm）：mammoth 转 HTML，保留标题 / 加粗 / 列表 / 表格
 * - Excel（.xlsx/.xlsm/.xlsb/.xls/…）：SheetJS 解析，按工作表切换渲染表格
 * - 其余（.doc / .ppt / .pptx / .odt / .rtf / .wps …）：没有可用的网页渲染方案，
 *   退化成信息卡 + 「用系统程序打开」
 *
 * 文件字节走 asset 协议 fetch（`convertFileSrc`，tauri.conf 的 assetProtocol.scope 是 `**\/*`），
 * 两个解析库都是动态 import —— 只有真的预览到对应格式时才把这坨体积加载进来。
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { convertFileSrc } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';
import {
  AlertTriangle, Download, ExternalLink, FileSpreadsheet, FileText,
  FolderOpen, Loader2, Presentation,
} from 'lucide-react';

/** 能被 mammoth 真正解析的 Word 格式（本质都是 OOXML zip） */
const WORD_RENDERABLE = ['docx', 'docm', 'dotx', 'dotm'];
/** 能被 SheetJS 真正解析的表格格式 */
const EXCEL_RENDERABLE = ['xlsx', 'xlsm', 'xlsb', 'xls', 'xlt', 'xltx', 'xltm', 'ods'];

/** 表格预览的行列上限：整张百万行的表塞进 DOM 会把预览页拖死 */
const MAX_TABLE_ROWS = 300;
const MAX_TABLE_COLS = 60;
/** 超过这个体积就不解析了，直接劝用户用外部程序打开 */
const MAX_PARSE_BYTES = 40 * 1024 * 1024;

function extOf(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 取回文件原始字节：asset 协议 + fetch，比让 Rust 端 base64 一遍再传回来省一半开销 */
async function readArrayBuffer(path: string): Promise<ArrayBuffer> {
  const res = await fetch(convertFileSrc(path));
  if (!res.ok) throw new Error(`读取文件失败：HTTP ${res.status}`);
  return res.arrayBuffer();
}

const OfficeActionCard: React.FC<{
  path: string;
  name: string;
  size: number;
  note?: string;
  error?: string | null;
  onOpenExternal: (path: string) => void;
  onReveal: (path: string) => void;
  onSaveAs: (path: string) => void;
}> = ({ path, name, size, note, error, onOpenExternal, onReveal, onSaveAs }) => {
  const { t } = useTranslation();
  const ext = extOf(name);
  const Icon = EXCEL_RENDERABLE.includes(ext) ? FileSpreadsheet
    : ['ppt', 'pptx', 'pptm', 'pot', 'potx', 'pps', 'ppsx', 'odp', 'dps', 'dpt'].includes(ext) ? Presentation
      : FileText;
  return (
    <div className="codex-office-card">
      <Icon size={30} strokeWidth={1.3} className="codex-office-card-icon" />
      <div className="codex-office-card-name" title={path}>{name}</div>
      <div className="codex-office-card-meta">
        {ext ? ext.toUpperCase() : t('mind_inspector.code_office_unknown_type')} · {formatSize(size)}
      </div>
      {note && <div className="codex-office-card-note">{note}</div>}
      {error && (
        <div className="codex-office-card-error">
          <AlertTriangle size={12} />
          <span>{error}</span>
        </div>
      )}
      <div className="codex-office-card-actions">
        <button type="button" className="primary" onClick={() => onOpenExternal(path)}>
          <ExternalLink size={12} />
          {t('mind_inspector.code_office_open_external')}
        </button>
        <button type="button" onClick={() => onReveal(path)}>
          <FolderOpen size={12} />
          {t('mind_inspector.code_office_reveal')}
        </button>
        <button type="button" onClick={() => onSaveAs(path)}>
          <Download size={12} />
          {t('mind_inspector.code_tab_menu_save_as')}
        </button>
      </div>
    </div>
  );
};

const OfficePreview: React.FC<{
  path: string;
  name: string;
  size: number;
  onOpenExternal: (path: string) => void;
  onReveal: (path: string) => void;
  onSaveAs: (path: string) => void;
}> = ({ path, name, size, onOpenExternal, onReveal, onSaveAs }) => {
  const { t } = useTranslation();
  const ext = extOf(name);
  const mode: 'word' | 'excel' | 'other' =
    WORD_RENDERABLE.includes(ext) ? 'word'
      : EXCEL_RENDERABLE.includes(ext) ? 'excel'
        : 'other';

  const [loading, setLoading] = useState(mode !== 'other');
  const [error, setError] = useState<string | null>(null);
  /** Word：mammoth 产出的 HTML（已过 DOMPurify） */
  const [docHtml, setDocHtml] = useState('');
  /** Excel：工作表名 + 各自的二维单元格数组 */
  const [sheets, setSheets] = useState<{ name: string; rows: string[][]; truncated: boolean }[]>([]);
  const [activeSheet, setActiveSheet] = useState(0);

  /** 组件卸载 / 换文件后丢弃迟到的解析结果 */
  const aliveRef = useRef(true);

  useEffect(() => {
    aliveRef.current = true;
    return () => { aliveRef.current = false; };
  }, []);

  const parse = useCallback(async () => {
    if (mode === 'other') return;
    if (size > MAX_PARSE_BYTES) {
      setLoading(false);
      setError(t('mind_inspector.code_office_too_large', { size: formatSize(size) }));
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const buffer = await readArrayBuffer(path);
      if (!aliveRef.current) return;

      if (mode === 'word') {
        // mammoth 的浏览器包是 UMD，默认导出挂在 default 上，取不到就退回模块本身
        const mod = await import('mammoth');
        const mammoth = (mod as unknown as { default?: unknown }).default ?? mod;
        const result = await (mammoth as {
          convertToHtml: (input: { arrayBuffer: ArrayBuffer }) => Promise<{ value: string; messages: unknown[] }>;
        }).convertToHtml({ arrayBuffer: buffer });
        if (!aliveRef.current) return;
        setDocHtml(DOMPurify.sanitize(result.value, { USE_PROFILES: { html: true } }));
      } else {
        const XLSX = await import('xlsx');
        const wb = XLSX.read(new Uint8Array(buffer), { type: 'array' });
        const parsed = wb.SheetNames.map((sheetName) => {
          const ws = wb.Sheets[sheetName];
          // raw: false → 取单元格的展示值（日期、千分位、百分比都按 Excel 里的样子给）
          const raw = XLSX.utils.sheet_to_json(ws, {
            header: 1, raw: false, blankrows: false, defval: '',
          }) as unknown as unknown[][];
          const rows = raw.slice(0, MAX_TABLE_ROWS).map((row) =>
            (Array.isArray(row) ? row : [row]).slice(0, MAX_TABLE_COLS).map((c) => (c == null ? '' : String(c))),
          );
          return { name: sheetName, rows, truncated: raw.length > MAX_TABLE_ROWS };
        });
        if (!aliveRef.current) return;
        setSheets(parsed);
        setActiveSheet(0);
      }
    } catch (e) {
      if (aliveRef.current) setError(String(e));
    } finally {
      if (aliveRef.current) setLoading(false);
    }
  }, [mode, path, size, t]);

  useEffect(() => { void parse(); }, [parse]);

  const sheet = sheets[activeSheet];
  const grid = useMemo(() => (sheet ? sheet.rows : []), [sheet]);

  if (mode === 'other') {
    return (
      <OfficeActionCard
        path={path} name={name} size={size} error={error}
        note={t('mind_inspector.code_office_no_renderer', { ext: ext.toUpperCase() })}
        onOpenExternal={onOpenExternal} onReveal={onReveal} onSaveAs={onSaveAs}
      />
    );
  }

  if (loading) {
    return (
      <div className="codex-office-loading">
        <Loader2 size={16} className="codex-spin" />
        <span>{t('mind_inspector.code_office_parsing')}</span>
      </div>
    );
  }

  // 解析失败（例如后缀名对不上、文件损坏、加密文档）→ 退化成信息卡，至少还能用外部程序打开
  if (error) {
    return (
      <OfficeActionCard
        path={path} name={name} size={size} error={error}
        note={t('mind_inspector.code_office_parse_failed')}
        onOpenExternal={onOpenExternal} onReveal={onReveal} onSaveAs={onSaveAs}
      />
    );
  }

  if (mode === 'word') {
    if (!docHtml.trim()) {
      return (
        <OfficeActionCard
          path={path} name={name} size={size}
          note={t('mind_inspector.code_office_empty')}
          onOpenExternal={onOpenExternal} onReveal={onReveal} onSaveAs={onSaveAs}
        />
      );
    }
    return (
      <div className="codex-office-scroll">
        {/* mammoth 输出的是文档结构 HTML（h1/p/table/strong…），已 DOMPurify 过一遍 */}
        <div className="codex-office-doc" dangerouslySetInnerHTML={{ __html: docHtml }} />
      </div>
    );
  }

  // Excel
  if (!sheet || sheet.rows.length === 0) {
    return (
      <OfficeActionCard
        path={path} name={name} size={size}
        note={t('mind_inspector.code_office_empty')}
        onOpenExternal={onOpenExternal} onReveal={onReveal} onSaveAs={onSaveAs}
      />
    );
  }

  return (
    <div className="codex-office-sheet">
      {sheets.length > 1 && (
        <div className="codex-office-sheet-tabs" role="tablist">
          {sheets.map((s, i) => (
            <button
              key={`${s.name}-${i}`}
              type="button"
              role="tab"
              aria-selected={i === activeSheet}
              className={`codex-office-sheet-tab${i === activeSheet ? ' active' : ''}`}
              onClick={() => setActiveSheet(i)}
              title={s.name}
            >
              {s.name}
            </button>
          ))}
        </div>
      )}
      <div className="codex-office-scroll">
        <table className="codex-office-table">
          <tbody>
            {grid.map((row, r) => (
              <tr key={r}>
                {row.map((cell, c) => (
                  <td key={c} title={cell}>{cell}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {sheet.truncated && (
        <div className="codex-office-truncated">
          {t('mind_inspector.code_office_rows_truncated', { n: MAX_TABLE_ROWS })}
        </div>
      )}
    </div>
  );
};

export default OfficePreview;
