!include "nsDialogs.nsh"
!include "LogicLib.nsh"

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
  ${NSD_CreateLabel} 0 0 100% 34u "可选组件：3D 公寓与街区探索。安装后也可以在设置 → 通用中启用或禁用。"
  Pop $0
  ${NSD_CreateCheckbox} 0 42u 100% 18u "安装 3D 公寓插件"
  Pop $ApartmentPluginCheckbox
  ${If} $ApartmentPluginChoice != 0
    ${NSD_Check} $ApartmentPluginCheckbox
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function ApartmentPluginPageLeave
  ${NSD_GetState} $ApartmentPluginCheckbox $ApartmentPluginChoice
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  ; Silent installs include the module by default; /NOAPARTMENT omits it.
  ${If} $ApartmentPluginChoice == ""
    StrCpy $ApartmentPluginChoice 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/NOAPARTMENT" $0
  ${IfNot} ${Errors}
    StrCpy $ApartmentPluginChoice 0
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $ApartmentPluginChoice != 1
    RMDir /r "$INSTDIR\plugins\3d-apartment\ui"
    RMDir /r "$INSTDIR\plugins\3d-apartment\room"
    Delete "$INSTDIR\plugins\3d-apartment\plugin.json"
    RMDir "$INSTDIR\plugins\3d-apartment"
    RMDir "$INSTDIR\plugins"
  ${EndIf}
!macroend
