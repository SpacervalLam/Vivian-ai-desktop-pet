!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "${__FILEDIR__}\resource-installer.nsh"

!define APARTMENT_HOOK_DIR "${__FILEDIR__}"
!if /FileExists "${APARTMENT_HOOK_DIR}\apartment-package.nsh"
  !include "${APARTMENT_HOOK_DIR}\apartment-package.nsh"
!else
  !define APARTMENT_PACKAGE "Vivian-3D-Apartment-1.1.0.zip"
  !define APARTMENT_SHA256 ""
!endif

Var ApartmentPluginChoice
Var ApartmentPluginCheckbox

; The setup UI asks for the optional apartment module before the normal pages.
Page custom ApartmentPluginPage ApartmentPluginPageLeave

Function ApartmentPluginPage
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 42u "可选组件：将配套 ZIP 放在安装程序旁即可选择添加。未勾选的字体和贴纸包会保留已安装版本。"
  Pop $0
  ${NSD_CreateCheckbox} 0 52u 100% 18u "安装 3D 公寓插件"
  Pop $ApartmentPluginCheckbox
  ${If} $ApartmentPluginChoice == 1
    ${NSD_Check} $ApartmentPluginCheckbox
  ${EndIf}
  IfFileExists "$EXEDIR\${APARTMENT_PACKAGE}" +3 0
    EnableWindow $ApartmentPluginCheckbox 0
    StrCpy $ApartmentPluginChoice 0
  !insertmacro ResourceCheckbox FontsChoice FontsCheckbox "${FONTS_PACKAGE}" "手写字体" 74
  !insertmacro ResourceCheckbox StickersChoice StickersCheckbox "${STICKERS_PACKAGE}" "内置贴纸" 96
  nsDialogs::Show
FunctionEnd

Function ApartmentPluginPageLeave
  ${NSD_GetState} $ApartmentPluginCheckbox $ApartmentPluginChoice
  ${NSD_GetState} $FontsCheckbox $FontsChoice
  ${NSD_GetState} $StickersCheckbox $StickersChoice
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  !insertmacro ResourcePrepare FontsChoice FONTS "${FONTS_PACKAGE}"
  !insertmacro ResourcePrepare StickersChoice STICKERS "${STICKERS_PACKAGE}"
  ; Silent installs are core-only unless explicitly passed /APARTMENT.
  ${If} $ApartmentPluginChoice == ""
    StrCpy $ApartmentPluginChoice 0
  ${EndIf}
  ${GetOptions} $CMDLINE "/APARTMENT" $0
  ${IfNot} ${Errors}
    StrCpy $ApartmentPluginChoice 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/NOAPARTMENT" $0
  ${IfNot} ${Errors}
    StrCpy $ApartmentPluginChoice 0
  ${EndIf}
  ${If} $ApartmentPluginChoice == 1
    IfFileExists "$EXEDIR\${APARTMENT_PACKAGE}" +3 0
      MessageBox MB_OK|MB_ICONSTOP "找不到公寓组件包：${APARTMENT_PACKAGE}"
      Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Remove the retired optional pack when upgrading an earlier installation.
  RMDir /r "$INSTDIR\optional\games"
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=install-resources.ps1 "${APARTMENT_HOOK_DIR}\install-resources.ps1"
  File /oname=resource-manifest.json "${APARTMENT_HOOK_DIR}\..\..\src\optional-resources.json"
  !insertmacro ResourceInstall FontsChoice fonts "${FONTS_PACKAGE}" "${FONTS_SHA256}"
  !insertmacro ResourceInstall StickersChoice stickers "${STICKERS_PACKAGE}" "${STICKERS_SHA256}"
  SetOutPath "$INSTDIR"
  ${If} $ApartmentPluginChoice == 1
    InitPluginsDir
    SetOutPath "$PLUGINSDIR"
    File /oname=install-apartment.ps1 "${APARTMENT_HOOK_DIR}\install-apartment.ps1"
    ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "$PLUGINSDIR\install-apartment.ps1" -Package "$EXEDIR\${APARTMENT_PACKAGE}" -Destination "$INSTDIR\plugins\3d-apartment" -ExpectedHash "${APARTMENT_SHA256}"' $0
    SetOutPath "$INSTDIR"
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONSTOP "基础程序已安装，但公寓组件安装失败。请确认 ZIP 与此安装程序配套且未损坏，然后重新运行安装程序。"
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
  ${If} $ApartmentPluginChoice != 1
    RMDir /r "$INSTDIR\plugins\3d-apartment\ui"
    RMDir /r "$INSTDIR\plugins\3d-apartment\room"
    Delete "$INSTDIR\plugins\3d-apartment\plugin.json"
    RMDir "$INSTDIR\plugins\3d-apartment"
    RMDir "$INSTDIR\plugins"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  RMDir /r "$INSTDIR\optional\fonts"
  RMDir /r "$INSTDIR\optional\stickers"
  RMDir "$INSTDIR\optional"
  RMDir /r "$INSTDIR\plugins\3d-apartment"
  RMDir "$INSTDIR\plugins"
!macroend
