import { useTranslation } from 'react-i18next';
import { resourcePackInstalled, type ResourcePackId } from '../../utils/optionalResources';

export default function OptionalResourceSettings() {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const packs: [ResourcePackId, string, string][] = [
    ['fonts', '手写字体', 'Handwriting font'], ['stickers', '内置贴纸', 'Built-in stickers'],
  ];
  return <section aria-label={zh ? '可选资源包' : 'Optional resources'} style={{ padding: '12px 0' }}>
    <strong>{zh ? '可选资源包' : 'Optional resources'}</strong>
    {packs.map(([id, label, english]) => <div key={id} style={{ paddingTop: 6 }}>
      {zh ? label : english}：{resourcePackInstalled(id) ? (zh ? '已安装' : 'Installed') : (zh ? '未安装' : 'Not installed')}
    </div>)}
    <p style={{ opacity: .7, fontSize: 12 }}>{zh
      ? '将需要的资源 ZIP 放在安装程序旁，重新运行安装程序并勾选对应组件。安装后重启应用。未安装字体时使用系统字体；自定义贴纸仍可使用。'
      : 'Place the resource ZIPs beside Setup, select the components, then restart the app. System fonts and custom stickers work without these packs.'}</p>
  </section>;
}
