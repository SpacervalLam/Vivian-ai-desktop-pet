import { useTranslation } from 'react-i18next';
import { sectionTitleStyle } from './SettingsFields';
import OptionalResourceSettings from './OptionalResourceSettings';
import './ResourceLibrarySettings.css';

export default function ResourceLibrarySettings(){
 const {i18n}=useTranslation();const zh=i18n.language.startsWith('zh');
 return <section className="resource-library" aria-label={zh?'可选资源包':'Optional resources'}>
  <h3 style={sectionTitleStyle}>{zh?'可选资源包':'Optional resources'}</h3>
  <div className="resource-library-content">
  <section className="resource-library-optional">
   <OptionalResourceSettings hideHeading/>
  </section></div>
 </section>;
}
