/**
 * 网络检测弹窗
 *
 * 设置窗口「网络」页签的诊断结果面板，取代早期只有一行文字结果的「测试连接」。
 *
 * 结构与后端 `network::diagnose` 的报告一一对应：
 * - 「当前目标」三段：服务端点 / 目标主机 / 代理模式
 * - 「检测项」五项：代理检测 / Hosts 解析 / 服务连通性 / TCP 连接延迟 / 丢包率
 *
 * 后端只回事实（status + facts），所有标题、说明与右侧细节都在这里按当前界面
 * 语言组装 —— 加语言不用动 Rust。
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { AlertTriangle, Check, Minus, RefreshCw, X, XCircle } from 'lucide-react';

/** 单项检测结论 —— 与 Rust 的 `DiagnosisStatus` 的 snake_case 序列化对齐 */
type DiagStatus = 'pass' | 'warn' | 'fail' | 'skip';

/** 单项检测的事实数据 —— 与 Rust 的 `DiagnosisFacts` 对齐 */
interface DiagFacts {
  proxy_url?: string;
  hosts_ip?: string;
  resolved_ip?: string;
  http_status?: number;
  elapsed_ms?: number;
  loss_percent?: number;
  error?: string;
}

interface DiagItem {
  id: 'proxy' | 'hosts' | 'connectivity' | 'tcp' | 'packet_loss' | string;
  status: DiagStatus;
  facts: DiagFacts;
}

interface DiagTarget {
  endpoint: string;
  host: string;
  port: number;
  host_port: string;
  scheme: string;
  path: string;
  proxy_mode: string;
  force_direct: boolean;
  effective_proxy?: string;
  endpoint_proxy?: string;
}

interface DiagReport {
  checked_at: string;
  target: DiagTarget;
  items: DiagItem[];
  passed: number;
  warned: number;
  failed: number;
}

/** 状态徽章的配色与图标 */
const STATUS_STYLE: Record<DiagStatus, { bg: string; color: string; Icon: typeof Check }> = {
  pass: { bg: 'rgba(127, 160, 132, 0.16)', color: 'var(--panel-success)', Icon: Check },
  warn: { bg: 'rgba(216, 164, 92, 0.16)', color: 'var(--panel-warning)', Icon: AlertTriangle },
  fail: { bg: 'rgba(201, 106, 87, 0.16)', color: 'var(--panel-danger)', Icon: XCircle },
  skip: { bg: 'var(--panel-bg-active)', color: 'var(--panel-text-tertiary)', Icon: Minus },
};

const MONO: React.CSSProperties = {
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
};

/** 「当前目标」/「检测项」中的一行：左标题+说明，右值（可选徽章） */
const DiagRow: React.FC<{
  title: string;
  description: string;
  value?: string;
  status?: DiagStatus;
}> = ({ title, description, value, status }) => {
  const { t } = useTranslation();
  const badge = status ? STATUS_STYLE[status] : null;
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 20,
        padding: '13px 16px',
        borderRadius: 10,
        background: 'var(--panel-bg-surface)',
        border: '1px solid var(--panel-border-light)',
        marginBottom: 8,
      }}
    >
      <div style={{ minWidth: 0 }}>
        <div style={{ fontSize: 14, fontWeight: 600, color: 'var(--panel-text)' }}>{title}</div>
        <div
          style={{
            fontSize: 12,
            color: 'var(--panel-text-tertiary)',
            marginTop: 3,
            lineHeight: 1.45,
          }}
        >
          {description}
        </div>
      </div>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          flexShrink: 0,
          maxWidth: '58%',
        }}
      >
        {value && (
          <span
            style={{
              ...MONO,
              fontSize: 12.5,
              color: 'var(--panel-text-secondary)',
              textAlign: 'right',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
            title={value}
          >
            {value}
          </span>
        )}
        {badge && (
          <span
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 4,
              padding: '3px 10px',
              borderRadius: 999,
              background: badge.bg,
              color: badge.color,
              fontSize: 12,
              fontWeight: 600,
              whiteSpace: 'nowrap',
            }}
          >
            <badge.Icon size={12} strokeWidth={2.6} />
            {t(`config.diag_status_${status}`)}
          </span>
        )}
      </div>
    </div>
  );
};

/**
 * i18next 的 `t` 是带泛型重载的签名，直接当作函数参数传会因参数逆变对不上类型；
 * 这里退化成普通签名再显式转换，仅用于 describeItem 这类纯文案组装函数。
 */
type Translate = (key: string, opts?: Record<string, unknown>) => string;

/** 省略的占位符（取值确实拿不到时用，不能用 0 / ? 这类看起来像真数据的字符） */
const NO_VALUE = '—';

/** 把非空片段拼起来 —— 缺项时不留尾随分隔符和多余空格 */
function joinParts(parts: Array<string | undefined>, sep = '  '): string {
  return parts.filter((p): p is string => !!p).join(sep);
}

/** 依据检测项 id + 事实数据组装界面文案 */
function describeItem(
  item: DiagItem,
  t: Translate,
  target: DiagTarget
): { description: string; value?: string } {
  const f = item.facts;
  const ms = (v?: number) => (v === undefined ? '' : `${v} ms`);

  switch (item.id) {
    case 'proxy': {
      if (item.status === 'skip') {
        // 后端只在「没有可用代理」时给 skip，但原因取决于用户选的模式：
        // 直连 / 跟随系统却没读到代理地址 / 手动代理但地址为空。
        // 一律说成「当前为直连模式」会在后两种情况下直接说错 —— 用户明明选了系统代理。
        const description =
          target.proxy_mode === 'system'
            ? t('config.diag_proxy_skip_system_desc')
            : target.proxy_mode === 'custom'
              ? t('config.diag_proxy_skip_custom_desc')
              : t('config.diag_proxy_skip_desc');
        return { description, value: t('config.diag_proxy_none') };
      }
      const url = f.proxy_url ?? t('config.diag_proxy_no_url');
      if (item.status === 'pass') {
        return {
          description: t('config.diag_proxy_pass_desc'),
          value: joinParts([url, ms(f.elapsed_ms)], ' · '),
        };
      }
      // 失败原因必须带出来（连不上 / 超时 / 地址根本解析不了），
      // 只给 URL 的话这行和「代理服务不可用」同义反复，排查不下去
      return {
        description: t('config.diag_proxy_fail_desc'),
        value: joinParts([url, f.error], ' · '),
      };
    }
    case 'hosts': {
      if (item.status === 'warn') {
        // 有映射时两个 IP 都给出：被 hosts 钉到了哪个 IP、DNS 本来会解析到哪个
        return {
          description: t('config.diag_hosts_warn_desc'),
          value: joinParts([
            f.hosts_ip ?? NO_VALUE,
            f.resolved_ip ? `DNS ${f.resolved_ip}` : undefined,
          ], ' · '),
        };
      }
      if (item.status === 'fail') {
        return { description: t('config.diag_hosts_fail_desc'), value: f.error };
      }
      return {
        description: t('config.diag_hosts_pass_desc'),
        value: joinParts([t('config.diag_hosts_no_entry'), f.resolved_ip], ' · '),
      };
    }
    case 'connectivity': {
      if (item.status === 'fail') {
        return { description: t('config.diag_conn_fail_desc'), value: f.error };
      }
      const head = f.http_status
        ? t('config.diag_conn_http', { status: f.http_status })
        : t('config.diag_conn_ok');
      return {
        description:
          item.status === 'warn' ? t('config.diag_conn_warn_desc') : t('config.diag_conn_pass_desc'),
        value: joinParts([head, ms(f.elapsed_ms)]),
      };
    }
    case 'tcp': {
      if (item.status === 'pass') {
        return { description: t('config.diag_tcp_pass_desc'), value: ms(f.elapsed_ms) };
      }
      return {
        description:
          item.status === 'warn' ? t('config.diag_tcp_warn_desc') : t('config.diag_tcp_fail_desc'),
        value: joinParts([f.error, ms(f.elapsed_ms)], ' · '),
      };
    }
    case 'packet_loss': {
      if (item.status === 'skip') {
        return { description: t('config.diag_ping_skip_desc'), value: f.error };
      }
      if (item.status === 'fail') {
        return { description: t('config.diag_ping_fail_desc'), value: f.error };
      }
      // 拿不到丢包率就如实留空 —— 兜底成 0% 会凭空造出一个「零丢包」的好结论
      const loss =
        f.loss_percent === undefined
          ? undefined
          : t('config.diag_ping_loss', { percent: f.loss_percent });
      const description =
        f.loss_percent === 100
          ? t('config.diag_ping_blocked_desc')
          : item.status === 'warn'
            ? t('config.diag_ping_warn_desc')
            : t('config.diag_ping_pass_desc');
      return { description, value: joinParts([loss, ms(f.elapsed_ms)]) };
    }
    default:
      return { description: '', value: undefined };
  }
}

const NetworkDiagnosisDialog: React.FC<{
  open: boolean;
  onClose: () => void;
  /** 打开前把 UI 中可能尚未保存的网络配置同步到后端内存（由调用方提供） */
  beforeRun?: () => Promise<void>;
}> = ({ open, onClose, beforeRun }) => {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(false);
  const [report, setReport] = useState<DiagReport | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);

  /**
   * 顶部汇总直接从**正在渲染的那份 items** 里数出来。
   * 用后端给的计数也能算对，但那样"汇总写 3 项、下面列了 4 行"就成了两个
   * 数据源之间的事故 —— 渲染什么就数什么，两者不可能再对不上。
   */
  const counts = useMemo(() => {
    const acc: Record<DiagStatus, number> = { pass: 0, warn: 0, fail: 0, skip: 0 };
    for (const item of report?.items ?? []) {
      acc[item.status] = (acc[item.status] ?? 0) + 1;
    }
    return acc;
  }, [report]);

  // `beforeRun` 是调用方每次渲染新建的闭包，不能进 useCallback 依赖 ——
  // 否则 `run` 每帧换身份，下面"打开即检测"的 effect 会无限重跑。用 ref 兜住。
  const beforeRunRef = useRef(beforeRun);
  useEffect(() => {
    beforeRunRef.current = beforeRun;
  });

  const run = useCallback(async () => {
    setLoading(true);
    setFatal(null);
    try {
      // 先同步未保存的设置，保证测到的就是用户当前看到的配置
      if (beforeRunRef.current) await beforeRunRef.current();
      const result = await invoke<DiagReport>('diagnose_network');
      setReport(result);
    } catch (e) {
      setFatal(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // 打开即检测（每次打开都重新跑，避免展示上一轮的过期结论）
  useEffect(() => {
    if (open) void run();
  }, [open, run]);

  // Esc 关闭
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;

  const proxyModeLabel = (mode: string, forceDirect: boolean) => {
    const base = t(
      mode === 'system'
        ? 'config.diag_proxy_mode_system'
        : mode === 'custom'
          ? 'config.diag_proxy_mode_custom'
          : 'config.diag_proxy_mode_direct'
    );
    // 国内厂商域名运行时强制直连，代理配置对这个端点不生效 —— 不点明会让人以为检测错了
    return forceDirect ? `${base}${t('config.diag_force_direct_hint')}` : base;
  };

  const proxyValue = (target: DiagTarget) => {
    if (target.endpoint_proxy) return target.endpoint_proxy;
    if (target.force_direct) return t('config.diag_proxy_forced_direct');
    return t('config.diag_proxy_none');
  };

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 1000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'var(--panel-overlay)',
        backdropFilter: 'blur(12px) saturate(120%)',
        WebkitBackdropFilter: 'blur(12px) saturate(120%)',
      }}
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 720,
          maxWidth: '92vw',
          maxHeight: '86vh',
          display: 'flex',
          flexDirection: 'column',
          borderRadius: 16,
          overflow: 'hidden',
          background: 'var(--panel-surface)',
          border: '1.5px solid var(--panel-border)',
          boxShadow: 'var(--panel-shadow-elevated)',
        }}
      >
        {/* ── 头部 ── */}
        <div
          style={{
            display: 'flex',
            alignItems: 'flex-start',
            justifyContent: 'space-between',
            gap: 16,
            padding: '18px 22px 14px',
            borderBottom: '1px solid var(--panel-border-light)',
          }}
        >
          <div>
            <div style={{ fontSize: 20, fontWeight: 700, color: 'var(--panel-text)' }}>
              {t('config.diag_title')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginTop: 5 }}>
              {report
                ? t('config.diag_last_checked', { time: report.checked_at })
                : t('config.diag_checking')}
            </div>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexShrink: 0 }}>
            <button
              onClick={() => void run()}
              disabled={loading}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 6,
                padding: '7px 14px',
                borderRadius: 8,
                border: '1px solid var(--panel-border)',
                background: 'transparent',
                color: 'var(--panel-text)',
                fontSize: 13,
                fontFamily: 'inherit',
                cursor: loading ? 'not-allowed' : 'pointer',
                opacity: loading ? 0.55 : 1,
              }}
            >
              <RefreshCw
                size={14}
                strokeWidth={2.2}
                style={loading ? { animation: 'gptsovits-spin 0.8s linear infinite' } : undefined}
              />
              {loading ? t('config.diag_checking') : t('config.diag_recheck')}
            </button>
            <button
              onClick={onClose}
              aria-label={t('config.diag_close')}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 30,
                height: 30,
                borderRadius: 8,
                border: 'none',
                background: 'transparent',
                color: 'var(--panel-text-secondary)',
                cursor: 'pointer',
              }}
            >
              <X size={17} strokeWidth={2.2} />
            </button>
          </div>
        </div>

        {/* ── 正文 ── */}
        <div style={{ overflowY: 'auto', padding: '16px 22px 22px' }}>
          {fatal ? (
            <div
              style={{
                padding: '14px 16px',
                borderRadius: 10,
                background: 'rgba(201, 106, 87, 0.12)',
                border: '1px solid rgba(201, 106, 87, 0.3)',
                color: 'var(--panel-danger)',
                fontSize: 13,
                ...MONO,
                wordBreak: 'break-all',
              }}
            >
              {fatal}
            </div>
          ) : !report ? (
            <div
              style={{
                padding: '40px 0',
                textAlign: 'center',
                fontSize: 13,
                color: 'var(--panel-text-tertiary)',
              }}
            >
              {t('config.diag_checking')}
            </div>
          ) : (
            <>
              {/* 当前目标 */}
              <div
                style={{
                  fontSize: 13,
                  fontWeight: 600,
                  color: 'var(--panel-text-secondary)',
                  margin: '2px 0 10px',
                }}
              >
                {t('config.diag_section_target')}
              </div>
              <DiagRow
                title={t('config.diag_target_endpoint')}
                description={t('config.diag_target_endpoint_desc')}
                value={report.target.endpoint}
              />
              <DiagRow
                title={t('config.diag_target_host')}
                description={t('config.diag_target_host_desc', {
                  scheme: report.target.scheme.toUpperCase(),
                  path: report.target.path,
                })}
                value={report.target.host_port}
              />
              <DiagRow
                title={t('config.diag_target_proxy')}
                description={t('config.diag_target_proxy_desc')}
                value={`${proxyModeLabel(report.target.proxy_mode, report.target.force_direct)} · ${proxyValue(
                  report.target
                )}`}
              />

              {/* 检测项 */}
              <div
                style={{
                  display: 'flex',
                  alignItems: 'baseline',
                  justifyContent: 'space-between',
                  gap: 12,
                  fontSize: 13,
                  fontWeight: 600,
                  color: 'var(--panel-text-secondary)',
                  margin: '20px 0 10px',
                }}
              >
                <span>{t('config.diag_section_items')}</span>
                <span
                  style={{
                    fontSize: 11.5,
                    fontWeight: 500,
                    color: counts.fail > 0 ? 'var(--panel-danger)' : 'var(--panel-text-tertiary)',
                  }}
                >
                  {t(
                    counts.skip > 0
                      ? 'config.diag_summary_with_skipped'
                      : 'config.diag_summary',
                    {
                      passed: counts.pass,
                      warned: counts.warn,
                      failed: counts.fail,
                      skipped: counts.skip,
                    }
                  )}
                </span>
              </div>
              {report.items.map((item) => {
                const { description, value } = describeItem(
                  item,
                  t as unknown as Translate,
                  report.target
                );
                return (
                  <DiagRow
                    key={item.id}
                    title={t(`config.diag_item_${item.id}`)}
                    description={description}
                    value={value}
                    status={item.status}
                  />
                );
              })}
            </>
          )}
        </div>
      </div>
    </div>
  );
};

export default NetworkDiagnosisDialog;
