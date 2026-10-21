/**
 * 聊天专属角色的模型设置面板。
 *
 * 这个角色只在聊天窗口里存在，所以它的模型路由（`routing_matrix.chat_partner`）
 * 不放进通用设置页——配置入口就在她自己身上：点头像，或私聊右上角菜单。
 *
 * 未配置时该路由留空，请求沿 catalog 的 fallback 链退回 `companion`；
 * 面板里显式展示"当前跟随 companion 的哪个模型"，避免用户以为默认是她自己的配置。
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import { useProviderPresets } from './settings/ModelSettings';

interface PartnerModel {
  provider_type?: string;
  model?: string;
  endpoint?: string;
  api_key?: string;
  temperature?: number | null;
  max_tokens?: number | null;
  context_window?: number | null;
}

interface PartnerModelState {
  route: string;
  configured: boolean;
  model: PartnerModel;
  fallback_route: string;
  fallback_model?: PartnerModel | null;
}

interface Props {
  partnerId: string;
  partnerName: string;
  onClose: () => void;
}

const labelStyle: React.CSSProperties = {
  fontSize: 11,
  color: 'var(--wx-text-secondary)',
  marginBottom: 4,
  display: 'block',
};

const fieldStyle: React.CSSProperties = {
  width: '100%',
  boxSizing: 'border-box',
  padding: '8px 10px',
  borderRadius: 8,
  border: '0.5px solid var(--wx-border)',
  background: 'var(--wx-search-bg)',
  color: 'var(--wx-text)',
  fontSize: 13,
  fontFamily: 'inherit',
  outline: 'none',
};

const ChatPartnerModelSheet: React.FC<Props> = ({ partnerId, partnerName, onClose }) => {
  const { t } = useTranslation();
  const presets = useProviderPresets().filter((preset) => preset.providerType !== 'jev');

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [state, setState] = useState<PartnerModelState | null>(null);

  /** 表单字段（未配置时先留空，用户一填就覆盖） */
  const [providerType, setProviderType] = useState('');
  const [model, setModel] = useState('');
  const [endpoint, setEndpoint] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [temperature, setTemperature] = useState('');
  const [maxTokens, setMaxTokens] = useState('');

  const load = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const result = await invoke<PartnerModelState>('get_chat_partner_model', { partnerId });
      setState(result);
      const source = result.configured ? result.model : null;
      // 未配置时不预填任何厂商：预填会让用户以为已经在用 GPT
      setProviderType(source?.provider_type || '');
      setModel(source?.model || '');
      setEndpoint(source?.endpoint || '');
      setApiKey(source?.api_key || '');
      setTemperature(source?.temperature != null ? String(source.temperature) : '');
      setMaxTokens(source?.max_tokens != null ? String(source.max_tokens) : '');
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    } finally {
      setLoading(false);
    }
  }, [partnerId]);

  useEffect(() => { void load(); }, [load]);

  /** 当前厂商预设：先按 endpoint 精确匹配（同一个 openai 兼容协议下厂商很多），再退回协议 */
  const matchPreset = useCallback(
    (type: string, url: string) => presets.find((preset) => preset.providerType === type && preset.endpoint === url)
      ?? presets.find((preset) => preset.providerType === type),
    [presets],
  );
  const activePreset = useMemo(
    () => matchPreset(providerType, endpoint),
    [matchPreset, providerType, endpoint],
  );
  /** 下拉当前值：未选择时是空字符串（显示占位项），否则是命中的预设 id 或 custom */
  const presetSelectValue = providerType === 'custom' ? 'custom' : (activePreset?.id ?? '');

  const applyPreset = (presetId: string) => {
    const preset = presets.find((item) => item.id === presetId);
    if (!preset) return;
    setProviderType(preset.providerType);
    setEndpoint(preset.endpoint);
    if (preset.defaultModel) setModel(preset.defaultModel);
    if (preset.suggestedMaxTokens) setMaxTokens(String(preset.suggestedMaxTokens));
  };

  const save = async (next: PartnerModel | null) => {
    setSaving(true);
    setError('');
    try {
      await invoke('set_chat_partner_model', {
        partnerId,
        model: next
          ? {
            provider_type: next.provider_type ?? providerType,
            model: (next.model ?? model).trim(),
            endpoint: (next.endpoint ?? endpoint).trim(),
            api_key: next.api_key ?? apiKey,
            temperature: next.temperature === undefined
              ? (temperature.trim() ? Number(temperature) : null)
              : next.temperature,
            max_tokens: next.max_tokens === undefined
              ? (maxTokens.trim() ? Number(maxTokens) : null)
              : next.max_tokens,
            context_window: next.context_window ?? activePreset?.contextWindow ?? null,
          }
          : null,
      });
      await emit('toast:show', {
        message: next ? t('chat.partner_model_saved') : t('chat.partner_model_cleared'),
        type: 'success',
        duration: 2400,
        key: `partner-model-${Date.now()}`,
      });
      await load();
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    } finally {
      setSaving(false);
    }
  };

  const fallbackLabel = (() => {
    const fb = state?.fallback_model;
    if (!fb) return '';
    const routeModel = (fb.model ?? '').trim();
    const routeProvider = (fb.provider_type ?? '').trim();
    if (!routeModel) return '';
    return routeProvider ? `${routeModel} · ${routeProvider}` : routeModel;
  })();

  return (
    <div
      onClick={onClose}
      style={{
        position: 'absolute', inset: 0, zIndex: 9998,
        background: 'rgba(0, 0, 0, 0.42)',
        display: 'flex', alignItems: 'flex-end', justifyContent: 'center',
        animation: 'fadeIn 0.16s ease',
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: '100%', maxHeight: '86%', overflowY: 'auto',
          background: 'var(--wx-bg-surface)',
          borderRadius: '18px 18px 0 0',
          borderTop: '0.5px solid var(--wx-border)',
          padding: '14px 16px 18px',
          boxShadow: '0 -12px 32px rgba(0, 0, 0, 0.28)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 4 }}>
          <span style={{ fontSize: 15, fontWeight: 600, color: 'var(--wx-text)' }}>
            {t('chat.partner_model_title', { name: partnerName })}
          </span>
          <button
            onClick={onClose}
            aria-label={t('chat.btn_cancel', { defaultValue: '取消' })}
            style={{
              marginLeft: 'auto', border: 'none', background: 'transparent',
              color: 'var(--wx-text-secondary)', fontSize: 13, cursor: 'pointer', fontFamily: 'inherit',
            }}
          >
            {t('chat.btn_cancel', { defaultValue: '取消' })}
          </button>
        </div>
        <div style={{ fontSize: 11, lineHeight: 1.5, color: 'var(--wx-text-tertiary)', marginBottom: 12 }}>
          {state?.configured
            ? t('chat.partner_model_hint_own', { route: state.route })
            : t('chat.partner_model_hint_follow', {
              route: state?.fallback_route ?? 'companion',
              model: fallbackLabel || t('chat.partner_model_unknown', { defaultValue: '未配置' }),
            })}
        </div>

        {loading ? (
          <div style={{ fontSize: 12, color: 'var(--wx-text-tertiary)', padding: '18px 0' }}>
            {t('chat.partner_model_loading')}
          </div>
        ) : (
          <>
            <div style={{ marginBottom: 10 }}>
              <label style={labelStyle} htmlFor="partner-model-provider">{t('chat.partner_model_provider')}</label>
              <select
                id="partner-model-provider"
                value={presetSelectValue}
                onChange={(e) => {
                  if (e.target.value === 'custom') { setProviderType('custom'); return; }
                  applyPreset(e.target.value);
                }}
                style={fieldStyle}
              >
                <option value="" disabled>{t('chat.partner_model_provider_placeholder')}</option>
                {presets.map((preset) => (
                  <option key={preset.id} value={preset.id}>
                    {preset.label ?? (preset.labelKey ? t(preset.labelKey) : preset.providerType)}
                  </option>
                ))}
                <option value="custom">{t('chat.partner_model_provider_custom')}</option>
              </select>
            </div>

            <div style={{ marginBottom: 10 }}>
              <label style={labelStyle} htmlFor="partner-model-name">{t('chat.partner_model_model')}</label>
              <input
                id="partner-model-name"
                list="partner-model-suggestions"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder={activePreset?.defaultModel ?? t('chat.partner_model_placeholder_model')}
                style={fieldStyle}
              />
              <datalist id="partner-model-suggestions">
                {(activePreset?.mainModels ?? []).map((name) => <option key={name} value={name} />)}
              </datalist>
            </div>

            <div style={{ marginBottom: 10 }}>
              <label style={labelStyle} htmlFor="partner-model-endpoint">{t('chat.partner_model_endpoint')}</label>
              <input
                id="partner-model-endpoint"
                value={endpoint}
                onChange={(e) => setEndpoint(e.target.value)}
                placeholder={activePreset?.endpoint ?? t('chat.partner_model_placeholder_endpoint')}
                style={fieldStyle}
              />
            </div>

            <div style={{ marginBottom: 10 }}>
              <label style={labelStyle} htmlFor="partner-model-key">{t('chat.partner_model_api_key')}</label>
              <input
                id="partner-model-key"
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder={t('chat.partner_model_api_key_placeholder')}
                style={fieldStyle}
              />
            </div>

            <div style={{ display: 'flex', gap: 10, marginBottom: 12 }}>
              <div style={{ flex: 1 }}>
                <label style={labelStyle} htmlFor="partner-model-temp">{t('chat.partner_model_temperature')}</label>
                <input
                  id="partner-model-temp"
                  inputMode="decimal"
                  value={temperature}
                  onChange={(e) => setTemperature(e.target.value)}
                  placeholder={t('chat.partner_model_inherit')}
                  style={fieldStyle}
                />
              </div>
              <div style={{ flex: 1 }}>
                <label style={labelStyle} htmlFor="partner-model-maxtokens">{t('chat.partner_model_max_tokens')}</label>
                <input
                  id="partner-model-maxtokens"
                  inputMode="numeric"
                  value={maxTokens}
                  onChange={(e) => setMaxTokens(e.target.value)}
                  placeholder={t('chat.partner_model_inherit')}
                  style={fieldStyle}
                />
              </div>
            </div>

            {error && (
              <div style={{ fontSize: 11, color: 'var(--wx-danger, #ff453a)', marginBottom: 10, lineHeight: 1.5 }}>
                {error}
              </div>
            )}

            <div style={{ display: 'flex', gap: 10 }}>
              <button
                onClick={() => void save(null)}
                disabled={saving || !state?.configured}
                style={{
                  flex: '0 0 auto', padding: '9px 14px', borderRadius: 10,
                  border: '0.5px solid var(--wx-border)', background: 'transparent',
                  color: state?.configured ? 'var(--wx-text)' : 'var(--wx-text-tertiary)',
                  fontSize: 13, fontFamily: 'inherit',
                  cursor: saving || !state?.configured ? 'default' : 'pointer',
                }}
              >
                {t('chat.partner_model_clear')}
              </button>
              <button
                onClick={() => void save({})}
                disabled={saving || !model.trim() || !endpoint.trim()}
                style={{
                  flex: 1, padding: '9px 14px', borderRadius: 10, border: 'none',
                  background: saving || !model.trim() || !endpoint.trim()
                    ? 'var(--wx-bg-active)'
                    : 'var(--wx-accent, #07c160)',
                  color: saving || !model.trim() || !endpoint.trim()
                    ? 'var(--wx-text-tertiary)'
                    : '#fff',
                  fontSize: 13, fontWeight: 600, fontFamily: 'inherit',
                  cursor: saving || !model.trim() || !endpoint.trim() ? 'default' : 'pointer',
                }}
              >
                {saving ? t('chat.partner_model_saving') : t('chat.partner_model_save')}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
};

export default ChatPartnerModelSheet;
