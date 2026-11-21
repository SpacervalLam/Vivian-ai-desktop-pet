/**
 * ASR 后端使用说明书（弹窗）
 *
 * 与 TTS 说明书共用 `EngineHelpDialog` 外壳，这里只把
 * `config.asr_help_<engine>_*` 的文案摊成 `EngineHelpDoc[]`。
 *
 * 引擎顺序与设置页「识别引擎」下拉一致；新增后端时同样要补
 * BACKEND_KEYS / FIELD_KEYS / PREP_COUNTS / FAQ_COUNTS 与三语文案。
 */

import React, { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import EngineHelpDialog, { type EngineHelpDoc } from './EngineHelpDialog';

export type AsrBackendKey = 'winrt' | 'whisper' | 'azure' | 'aliyun' | 'openai_whisper';

interface AsrHelpDialogProps {
  open: boolean;
  /** 初始定位的后端，默认 winrt */
  initialBackend?: AsrBackendKey;
  onClose: () => void;
}

const BACKEND_KEYS: AsrBackendKey[] = ['winrt', 'whisper', 'azure', 'aliyun', 'openai_whisper'];

/** 各后端的字段键列表（与设置页展示的字段顺序对齐） */
const FIELD_KEYS: Record<AsrBackendKey, string[]> = {
  winrt: ['engine', 'language', 'silence_timeout'],
  whisper: ['server_url', 'api_format', 'api_key', 'max_audio_seconds', 'language'],
  azure: ['speech_key', 'speech_region', 'conversation_mode', 'max_audio_seconds', 'language'],
  aliyun: ['app_key', 'access_key_id', 'access_key_secret', 'max_audio_seconds', 'language'],
  openai_whisper: ['api_key', 'base_url', 'max_audio_seconds', 'language'],
};

/** 各后端「准备工作」条目数 */
const PREP_COUNTS: Record<AsrBackendKey, number> = {
  winrt: 3,
  whisper: 6,
  azure: 4,
  aliyun: 5,
  openai_whisper: 3,
};

/** 各后端 FAQ 条目数 */
const FAQ_COUNTS: Record<AsrBackendKey, number> = {
  winrt: 3,
  whisper: 4,
  azure: 3,
  aliyun: 3,
  openai_whisper: 3,
};

const AsrHelpDialog: React.FC<AsrHelpDialogProps> = ({ open, initialBackend = 'winrt', onClose }) => {
  const { t, i18n } = useTranslation();

  const docs = useMemo<EngineHelpDoc[]>(
    () =>
      BACKEND_KEYS.map((k) => ({
        key: k,
        name: t(`config.asr_help_${k}_name`),
        what: t(`config.asr_help_${k}_what`),
        prep: Array.from({ length: PREP_COUNTS[k] }, (_, i) =>
          t(`config.asr_help_${k}_prep_${i + 1}`),
        ),
        fields: FIELD_KEYS[k].map((f) => ({
          name: f,
          desc: t(`config.asr_help_${k}_fields_${f}`),
        })),
        recommend: t(`config.asr_help_${k}_recommend`),
        faq: Array.from({ length: FAQ_COUNTS[k] }, (_, i) => ({
          q: t(`config.asr_help_${k}_faq_q${i + 1}`),
          a: t(`config.asr_help_${k}_faq_a${i + 1}`),
        })),
      })),
    // 依赖 language：切语言后不重算的话，弹窗会停在旧语言
    [t, i18n.language],
  );

  return (
    <EngineHelpDialog
      open={open}
      initialKey={initialBackend}
      docs={docs}
      title={t('config.asr_help_title')}
      subtitle={t('config.asr_help_subtitle')}
      listLabel={t('config.asr_help_list_label')}
      sectionLabels={{
        what: t('config.asr_help_section_what'),
        prep: t('config.asr_help_section_prep'),
        fields: t('config.asr_help_section_fields'),
        recommend: t('config.asr_help_section_recommend'),
        faq: t('config.asr_help_section_faq'),
      }}
      onClose={onClose}
    />
  );
};

export default AsrHelpDialog;
