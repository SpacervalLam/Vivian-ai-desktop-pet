!include "nsDialogs.nsh"
!include "LogicLib.nsh"

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
  ${NSD_CreateLabel} 0 0 100% 42u "可选组件：3D 公寓与街区探索。将 ${APARTMENT_PACKAGE} 放在安装程序旁即可选择安装。安装后可在设置 → 通用中启用或禁用。"
  Pop $0
  ${NSD_CreateCheckbox} 0 52u 100% 18u "安装 3D 公寓插件"
  Pop $ApartmentPluginCheckbox
  ${If} $ApartmentPluginChoice == 1
    ${NSD_Check} $ApartmentPluginCheckbox
  ${EndIf}
  IfFileExists "$EXEDIR\${APARTMENT_PACKAGE}" +3 0
    EnableWindow $ApartmentPluginCheckbox 0
    StrCpy $ApartmentPluginChoice 0
  nsDialogs::Show
FunctionEnd

Function ApartmentPluginPageLeave
  ${NSD_GetState} $ApartmentPluginCheckbox $ApartmentPluginChoice
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
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
  ${If} $ApartmentPluginChoice == 1
    InitPluginsDir
    SetOutPath "$PLUGINSDIR"
    File /oname=install-apartment.ps1 "${APARTMENT_HOOK_DIR}\install-apartment.ps1"
    ExecWait '$"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe$" -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File $"$PLUGINSDIR\install-apartment.ps1$" -Package $"$EXEDIR\${APARTMENT_PACKAGE}$" -Destination $"$INSTDIR\plugins\3d-apartment$" -ExpectedHash $"${APARTMENT_SHA256}$"' $0
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
  RMDir /r "$INSTDIR\plugins\3d-apartment"
  RMDir "$INSTDIR\plugins"
!macroend
