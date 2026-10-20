/**
 * TTS 后端使用说明书（弹窗）
 *
 * 只负责把 `config.tts_help_<engine>_*` 的文案摊成 `EngineHelpDoc[]`，
 * 排版与交互全部交给 `EngineHelpDialog`。
 *
 * 引擎顺序与设置页「合成引擎」下拉保持一致，方便对照。
 * 新增后端时必须同时补：本文件的 BACKEND_KEYS / fieldKeys / prepCounts / faqCounts
 * 与三语文案，否则该页签缺失或界面会直接渲染出原始 key。
 */

import React, { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import EngineHelpDialog, { type EngineHelpDoc } from './EngineHelpDialog';

export type TtsBackendKey =
  | 'edgetts'
  | 'azure'
  | 'gptsovits'
  | 'fishspeech'
  | 'minimax'
  | 'doubao'
  | 'mimo'
  | 'elevenlabs';

interface TtsHelpDialogProps {
  open: boolean;
  /** 初始定位的后端，默认 edgetts */
  initialBackend?: TtsBackendKey;
  onClose: () => void;
}

const BACKEND_KEYS: TtsBackendKey[] = [
  'edgetts',
  'azure',
  'gptsovits',
  'fishspeech',
  'minimax',
  'doubao',
  'mimo',
  'elevenlabs',
];

/** 各后端的字段键列表（与设置页展示的字段顺序对齐） */
const FIELD_KEYS: Record<TtsBackendKey, string[]> = {
  edgetts: ['rate', 'volume'],
  azure: ['key', 'region', 'style', 'style_degree', 'role', 'pitch', 'output_format'],
  gptsovits: ['url', 'ref_audio', 'prompt_text'],
  fishspeech: ['url', 'key', 'character', 'format', 'ref_audio', 'ref_text'],
  minimax: ['key', 'voice_id', 'model', 'format', 'sample_rate'],
  doubao: ['appid', 'access_token', 'cluster', 'voice_type', 'format', 'sample_rate'],
  mimo: ['key', 'voice_audio', 'style_prompt', 'endpoint'],
  elevenlabs: ['key', 'voice_id', 'model', 'output_format', 'voice_settings', 'endpoint'],
};

/** 各后端「准备工作」条目数 */
const PREP_COUNTS: Record<TtsBackendKey, number> = {
  edgetts: 3,
  azure: 4,
  gptsovits: 5,
  fishspeech: 5,
  minimax: 5,
  doubao: 5,
  mimo: 5,
  elevenlabs: 5,
};

/** 各后端 FAQ 条目数 */
const FAQ_COUNTS: Record<TtsBackendKey, number> = {
  edgetts: 2,
  azure: 4,
  gptsovits: 3,
  fishspeech: 3,
  minimax: 3,
  doubao: 3,
  mimo: 3,
  elevenlabs: 3,
};

const TtsHelpDialog: React.FC<TtsHelpDialogProps> = ({ open, initialBackend = 'edgetts', onClose }) => {
  const { t, i18n } = useTranslation();

  const docs = useMemo<EngineHelpDoc[]>(
    () =>
      BACKEND_KEYS.map((k) => ({
        key: k,
        name: t(`config.tts_help_${k}_name`),
        what: t(`config.tts_help_${k}_what`),
        prep: Array.from({ length: PREP_COUNTS[k] }, (_, i) =>
          t(`config.tts_help_${k}_prep_${i + 1}`),
        ),
        fields: FIELD_KEYS[k].map((f) => ({
          name: f,
          desc: t(`config.tts_help_${k}_fields_${f}`),
        })),
        recommend: t(`config.tts_help_${k}_recommend`),
        faq: Array.from({ length: FAQ_COUNTS[k] }, (_, i) => ({
          q: t(`config.tts_help_${k}_faq_q${i + 1}`),
          a: t(`config.tts_help_${k}_faq_a${i + 1}`),
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
      title={t('config.tts_help_title')}
      subtitle={t('config.tts_help_subtitle')}
      listLabel={t('config.tts_help_list_label')}
      sectionLabels={{
        what: t('config.tts_help_section_what'),
        prep: t('config.tts_help_section_prep'),
        fields: t('config.tts_help_section_fields'),
        recommend: t('config.tts_help_section_recommend'),
        faq: t('config.tts_help_section_faq'),
      }}
      onClose={onClose}
    />
  );
};

export default TtsHelpDialog;
