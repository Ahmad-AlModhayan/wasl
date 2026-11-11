; WaslIsWixEntry HIVE KEY NAME PUBLISHER OUT: sets OUT to 1 when the uninstall entry KEY under HIVE
; is the old WiX (.msi) install of Wasl, else 0: DisplayName is NAME and Publisher is PUBLISHER,
; each compared on its own (ignoring case, as StrCmp does), and msiexec uninstalls it. Uses $R0
; and $R1. Needs LogicLib and StrFunc's StrCase and StrLoc.
; wasl-installer.Tests.ps1 builds it into a small test program.
!macro WaslIsWixEntry HIVE KEY NAME PUBLISHER OUT
  StrCpy ${OUT} 0
  ReadRegStr $R0 ${HIVE} "${KEY}" "DisplayName"
  ReadRegStr $R1 ${HIVE} "${KEY}" "Publisher"
  ${If} $R0 == "${NAME}"
  ${AndIf} $R1 == "${PUBLISHER}"
    ReadRegStr $R0 ${HIVE} "${KEY}" "UninstallString"
    ${StrCase} $R1 $R0 "L"
    ${StrLoc} $R0 $R1 "msiexec" ">"
    ${If} $R0 == 0
      StrCpy ${OUT} 1
    ${EndIf}
  ${EndIf}
!macroend
