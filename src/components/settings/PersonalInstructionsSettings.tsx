import { useTranslation } from 'react-i18next';

export const CUSTOM_INSTRUCTIONS_LIMIT = 16000;

export default function PersonalInstructionsSettings({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const { t } = useTranslation();
  return <div className="settings-instructions">
    <p className="settings-data-help" id="custom-instructions-description">{t('storageSettings.instructionsHelp')}</p>
    <textarea id="custom-instructions" aria-label={t('storageSettings.instructions')}
      aria-describedby="custom-instructions-description custom-instructions-save" value={value}
      onChange={e => onChange(e.target.value)} maxLength={CUSTOM_INSTRUCTIONS_LIMIT}
      placeholder={t('storageSettings.instructionsPlaceholder')} spellCheck={false} />
    <div className="settings-instructions-footer">
      <span id="custom-instructions-save">{t('storageSettings.instructionsSave')}</span>
      <span>{t('storageSettings.instructionsCount', { count: value.length, limit: CUSTOM_INSTRUCTIONS_LIMIT })}</span>
    </div>
  </div>;
}
