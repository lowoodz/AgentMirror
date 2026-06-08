; SafeRoute NSIS hooks — install CLI companion files and clean up on uninstall.

!macro SafeRoute_StopProcesses
  ExecWait 'taskkill /IM SafeRoute.exe /T' $0
  ExecWait 'taskkill /IM SecureModelRoute.exe /T' $0
  ExecWait 'taskkill /IM smr.exe /T' $0
  Sleep 1500
  ExecWait 'taskkill /F /IM SafeRoute.exe /T' $0
  ExecWait 'taskkill /F /IM SecureModelRoute.exe /T' $0
  ExecWait 'taskkill /F /IM smr.exe /T' $0
!macroend

!macro SafeRoute_InstallCliCompanion
  IfFileExists "$INSTDIR\resources\cli\smr.exe" 0 +10
    CreateDirectory "$PROFILE\.local\bin"
    CopyFiles /SILENT "$INSTDIR\resources\cli\smr.exe" "$PROFILE\.local\bin\smr.exe"
    CreateDirectory "$PROFILE\.local\etc\securemodelroute"
    IfFileExists "$PROFILE\.local\etc\securemodelroute\smr.yaml" +2 0
      CopyFiles /SILENT "$INSTDIR\resources\cli\smr.example.yaml" "$PROFILE\.local\etc\securemodelroute\smr.yaml"
    IfFileExists "$INSTDIR\resources\cli\post-install-cli.ps1" 0 +2
      ExecWait '"$\"powershell.exe$\" -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File \"$INSTDIR\resources\cli\post-install-cli.ps1$\"' $0
!macroend

!macro SafeRoute_RemoveCliCompanion
  IfFileExists "$INSTDIR\resources\cli\post-uninstall-cli.ps1" 0 +2
    ExecWait '"$\"powershell.exe$\" -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File \"$INSTDIR\resources\cli\post-uninstall-cli.ps1$\"' $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro SafeRoute_StopProcesses
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro SafeRoute_InstallCliCompanion
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro SafeRoute_StopProcesses
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro SafeRoute_RemoveCliCompanion
!macroend
