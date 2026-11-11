; Included by installer.nsi through `bundle.windows.nsis.installerHooks`, which is how Tauri hands
; the template an absolute path into this folder: the strings and the PowerShell helper live here.
!define WASL_NSIS_DIR "${__FILEDIR__}"
