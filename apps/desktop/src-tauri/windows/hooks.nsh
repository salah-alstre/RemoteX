; NSIS installer hooks: install, start and remove the RemoteX elevated-control Windows service.
; The service lives in the installation directory (Program Files), which only administrators can write to;
; the service refuses to grant anything from a user-writable location.

!macro NSIS_HOOK_PREINSTALL
  ; An earlier per-user installation (before the service existed) would sit next to this one: remove it,
  ; keeping the user's settings and device identity (they live in the user profile, not the install folder).
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteX" "UninstallString"
  ${If} $R0 != ""
    ExecWait '$R0 /S' $R1
  ${EndIf}
  ; Upgrading: stop and remove the old service so its executable can be replaced.
  ${If} ${FileExists} "$INSTDIR\remotex-service.exe"
    nsExec::Exec '"$INSTDIR\remotex-service.exe" uninstall'
    Pop $R1
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Creates the service (automatic start, restart on failure, restricted access list) and starts it.
  nsExec::ExecToLog '"$INSTDIR\remotex-service.exe" install'
  Pop $R1
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\remotex-service.exe" uninstall'
  Pop $R1
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
