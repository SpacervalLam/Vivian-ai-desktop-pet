!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "${__FILEDIR__}\resource-installer.nsh"
!define RESOURCE_HOOK_DIR "${__FILEDIR__}"
Page custom OptionalResourcesPage OptionalResourcesPageLeave
Function OptionalResourcesPage
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 42u "可选组件：将配套 ZIP 放在安装程序旁即可选择添加。未勾选的字体和贴纸包会保留已安装版本。"
  Pop $0
  !insertmacro ResourceCheckbox FontsChoice FontsCheckbox "${FONTS_PACKAGE}" "手写字体" 52
  !insertmacro ResourceCheckbox StickersChoice StickersCheckbox "${STICKERS_PACKAGE}" "内置贴纸" 74
  nsDialogs::Show
FunctionEnd
Function OptionalResourcesPageLeave
  ${NSD_GetState} $FontsCheckbox $FontsChoice
  ${NSD_GetState} $StickersCheckbox $StickersChoice
FunctionEnd
!macro NSIS_HOOK_PREINSTALL
  !insertmacro ResourcePrepare FontsChoice FONTS "${FONTS_PACKAGE}"
  !insertmacro ResourcePrepare StickersChoice STICKERS "${STICKERS_PACKAGE}"
!macroend
!macro NSIS_HOOK_POSTINSTALL
  RMDir /r "$INSTDIR\optional\games"
  RMDir /r "$INSTDIR\browser-runtime"
  RMDir /r "$INSTDIR\resources\browser-runtime"
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=install-resources.ps1 "${RESOURCE_HOOK_DIR}\install-resources.ps1"
  File /oname=resource-manifest.json "${RESOURCE_HOOK_DIR}\..\..\src\optional-resources.json"
  !insertmacro ResourceInstall FontsChoice fonts "${FONTS_PACKAGE}" "${FONTS_SHA256}"
  !insertmacro ResourceInstall StickersChoice stickers "${STICKERS_PACKAGE}" "${STICKERS_SHA256}"
  SetOutPath "$INSTDIR"
!macroend
!macro NSIS_HOOK_PREUNINSTALL
  RMDir /r "$INSTDIR\optional\fonts"
  RMDir /r "$INSTDIR\optional\stickers"
  RMDir "$INSTDIR\optional"
!macroend
