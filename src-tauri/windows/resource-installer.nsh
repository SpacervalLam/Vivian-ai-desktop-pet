; Resource archives stay next to Setup; only this installation bridge is embedded.
!include "${__FILEDIR__}\resource-packages.nsh"
Var FontsChoice
Var FontsCheckbox
Var StickersChoice
Var StickersCheckbox

!macro ResourceCheckbox Choice Checkbox Package Title Y
  ${NSD_CreateCheckbox} 0 ${Y}u 100% 18u "添加${Title}"
  Pop $${Checkbox}
  ${If} $${Choice} == 1
    ${NSD_Check} $${Checkbox}
  ${EndIf}
  IfFileExists "$EXEDIR\${Package}" +3 0
    EnableWindow $${Checkbox} 0
    StrCpy $${Choice} 0
!macroend

!macro ResourcePrepare Choice Switch Package
  ${If} $${Choice} == ""
    StrCpy $${Choice} 0
  ${EndIf}
  ${GetOptions} $CMDLINE "/${Switch}" $0
  ${IfNot} ${Errors}
    StrCpy $${Choice} 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/NO${Switch}" $0
  ${IfNot} ${Errors}
    StrCpy $${Choice} 0
  ${EndIf}
  ${If} $${Choice} == 1
    IfFileExists "$EXEDIR\${Package}" +3 0
      MessageBox MB_OK|MB_ICONSTOP "找不到可选资源包：${Package}"
      Abort
  ${EndIf}
!macroend

!macro ResourceInstall Choice ID Package Hash
  ${If} $${Choice} == 1
    ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "$PLUGINSDIR\install-resources.ps1" -Package "$EXEDIR\${Package}" -InstallRoot "$INSTDIR" -PackId "${ID}" -ExpectedHash "${Hash}" -ManifestPath "$PLUGINSDIR\resource-manifest.json"' $0
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONSTOP "基础程序已安装，但可选资源包安装失败：${Package}。请使用与安装程序配套的 ZIP 重试。"
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
!macroend
