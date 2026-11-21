/** Keep native webpage scrollbars consistent with CodeAgentPage.css. */
export function browserScrollbarCss(thumb: string, hover: string): string {
  return `
    html, body, * { scrollbar-width: auto !important; scrollbar-color: auto !important; }
    ::-webkit-scrollbar { width: 12px !important; height: 12px !important; }
    ::-webkit-scrollbar-track, ::-webkit-scrollbar-corner { background: transparent !important; }
    ::-webkit-scrollbar-button { display: none !important; width: 0 !important; height: 0 !important; }
    ::-webkit-scrollbar-thumb {
      border: 3px solid transparent !important; border-radius: 999px !important;
      background: ${thumb || 'rgba(59,52,40,.22)'} !important;
      background-clip: padding-box !important; min-height: 40px !important; min-width: 40px !important;
    }
    ::-webkit-scrollbar-thumb:hover, ::-webkit-scrollbar-thumb:active {
      background: ${hover || 'rgba(59,52,40,.34)'} !important;
      background-clip: padding-box !important;
    }
  `;
}
