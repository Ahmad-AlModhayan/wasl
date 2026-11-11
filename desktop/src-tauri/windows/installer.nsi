; Wasl's Windows installer (issue #204). Tauri fills in the handlebars values at `tauri build`; the rest
; started from Tauri 2.12's own NSIS template (crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi)
; and keeps its reinstall, WebView2 and updater (/UPDATE) behaviour. Tauri's template is
; Copyright 2019-2024 Tauri Programme within The Commons Conservancy, Apache-2.0 OR MIT; see
; desktop/THIRD-PARTY-NOTICES.md.
;
; Pages: language, welcome, licence (only when bundle.licenseFile is set; none today), install scope, reinstall/upgrade,
; components, folder, progress, finish. English and Arabic; Arabic is laid out right to left.
;
; Command line (silent and scripted setups):
;   /S              silent            /P            passive: progress only, no questions
;   /CURRENTUSER    just me (default for a new install; no administrator needed)
;   /ALLUSERS       everyone (needs an elevated prompt when silent; exits 740 otherwise)
;   /NOCLI          leave PATH alone  /NOSTARTMENU  no Start menu shortcut  /NS  no shortcuts at all
;   /DESKTOP        add a desktop shortcut          /NOAUTOSTART  don't start Wasl at sign-in
;   /LANG=1025      Arabic (1033 English)           /R            start Wasl after a silent install
;   /D=<folder>     install folder (must be last)
; An upgrade keeps the components chosen last time unless a switch says otherwise.
; Uninstaller: /S silent, keeps the data unless /DELETEDATA is given; finds its own scope, or takes
; /ALLUSERS or /CURRENTUSER.
;
; Signing: not done yet (needs a code-signing certificate). Setting `bundle.windows.signCommand` (or
; `certificateThumbprint`) in tauri.bundle.conf.json signs the app, wasld, wasl, this installer and,
; through UNINSTALLERSIGNCOMMAND below, the uninstaller.

Unicode true
ManifestDPIAware true
ManifestDPIAwareness PerMonitorV2

!if "{{compression}}" == "none"
  SetCompress off
!else
  SetCompressor /SOLID "{{compression}}"
!endif

{{#if signed_plugins_path}}
!addplugindir "{{signed_plugins_path}}"
{{/if}}

!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include LogicLib.nsh
!include Sections.nsh
!include nsDialogs.nsh
!include WinMessages.nsh
!include "utils.nsh"
!include "FileAssociation.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"
!include "StrFunc.nsh"
${StrCase}
${StrLoc}

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}
!ifndef WASL_NSIS_DIR
  !error "installerHooks must point at desktop/src-tauri/windows/wasl-hooks.nsh"
!endif

!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define HOMEPAGE "{{homepage}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define UNINSTALLERICON "{{uninstaller_icon}}"
!define UNINSTALLERHEADERIMAGE "{{uninstaller_header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define ADDITIONALPLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"
; wasld's data (wasl-ipc Paths::user_default) and the app's own folders (Tauri app_config_dir etc.)
!define WASLDATADIR "$LOCALAPPDATA\Wasl"
!define POWERSHELL "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
!define ERROR_ELEVATION_REQUIRED 740

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var WixMode
Var OldMainBinaryName
Var Scope          ; "user" or "all"
Var ScopeLocked    ; 1: chosen on the command line or by the elevated relaunch, no scope page
Var Elevated       ; 1: this is the relaunch that asked for administrator rights
Var ExistingScope  ; where an install was found: "user", "all" or ""
Var WasRunning     ; stop-in-dir's answer: 1 wasld, 2 the app, 3 both
Var FinishText
Var WixProduct     ; product code of an old .msi install to remove in SEC_CORE
Var RemoveOld      ; 1: the reinstall page chose "uninstall the old version first"
Var OtherUser      ; 1: elevated by another account (over the shoulder), not the person at the desktop
Var CliDesc

Name "${PRODUCTNAME}"
!if "${COPYRIGHT}" != ""
  BrandingText "${COPYRIGHT}"
!else
  BrandingText "${PRODUCTNAME} ${VERSION}"
!endif
OutFile "${OUTFILE}"

!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"
; A drive root (D:\) is a valid choice; the helper gets it as "D:\." (see WASLDIR).
AllowRootDirInstall true

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME} Setup"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!addplugindir "${ADDITIONALPLUGINSPATH}"

!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Always starts without administrator rights; "Everyone" relaunches itself elevated (ScopeLeave).
RequestExecutionLevel user

!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif
!if "${SIDEBARIMAGE}" != ""
  !define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
  !define MUI_UNWELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
!endif
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
  !define MUI_HEADERIMAGE_BITMAP "${HEADERIMAGE}"
  !define MUI_HEADERIMAGE_RIGHT
!else if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!endif
!if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_UNBITMAP "${UNINSTALLERHEADERIMAGE}"
!else if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_UNBITMAP "${HEADERIMAGE}"
!endif
!if "${UNINSTALLERICON}" != ""
  !define MUI_UNICON "${UNINSTALLERICON}"
!endif
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_SMALLDESC

!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"
!define MUI_LANGDLL_ALLLANGUAGES

; ---------------------------------------------------------------- installer pages
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassiveOrElevated
!define MUI_WELCOMEPAGE_TITLE "$(wWelcomeTitle)"
!define MUI_WELCOMEPAGE_TEXT "$(wWelcomeText)"
!insertmacro MUI_PAGE_WELCOME

!if "${LICENSE}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassiveOrElevated
  !define MUI_PAGE_CUSTOMFUNCTION_SHOW LicenceShow
  !insertmacro MUI_PAGE_LICENSE "$(wLicenceFile)"
!endif

Page custom ScopePage ScopeLeave

Var ReinstallPageCheck
Page custom PageReinstall PageLeaveReinstall

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!define MUI_PAGE_HEADER_SUBTEXT "$(wCompSub)"
!insertmacro MUI_PAGE_COMPONENTS

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY

!insertmacro MUI_PAGE_INSTFILES

!define MUI_FINISHPAGE_NOAUTOCLOSE
!define MUI_FINISHPAGE_TITLE "$(wFinishTitle)"
!define MUI_FINISHPAGE_TEXT "$FinishText"
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "$(wLaunch)"
!define MUI_FINISHPAGE_RUN_FUNCTION RunMainBinary
!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(wTryCli)"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION TryCli
!define MUI_PAGE_CUSTOMFUNCTION_PRE FinishPre
!define MUI_PAGE_CUSTOMFUNCTION_SHOW FinishShow
!insertmacro MUI_PAGE_FINISH

; ---------------------------------------------------------------- uninstaller pages
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState
Var DataShared
!define /ifndef WS_EX_LAYOUTRTL 0x00400000
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.ConfirmShow
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.ConfirmLeave
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; ---------------------------------------------------------------- languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
; .onInit runs the helper: keep it at the front of the solid archive, so reaching it does not unpack
; everything else first (which also shows a "Setup is loading" window).
ReserveFile "${WASL_NSIS_DIR}\wasl-installer.ps1"
{{#each language_files}}
  !include "{{this}}"
{{/each}}
!include "${WASL_NSIS_DIR}\strings.nsh"
!include "${WASL_NSIS_DIR}\wix-entry.nsh"
!if "${LICENSE}" != ""
  ; LICENSE (bundle.licenseFile) in each language's order: the Arabic installer shows the Arabic text first.
  !tempfile WASL_LICENCE_EN
  !tempfile WASL_LICENCE_AR
  !tempfile WASL_LICENCE_NSH
  !system '"$%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "${WASL_NSIS_DIR}\split-licence.ps1" "${LICENSE}" "${WASL_LICENCE_EN}" "${WASL_LICENCE_AR}" "${WASL_LICENCE_NSH}"' = 0
  !include "${WASL_LICENCE_NSH}"
  LicenseLangString wLicenceFile ${LANG_ENGLISH} "${WASL_LICENCE_EN}"
  LicenseLangString wLicenceFile ${LANG_ARABIC} "${WASL_LICENCE_AR}"
!endif

!if "${LICENSE}" != ""
!ifndef EM_SETPARAFORMAT
  !define EM_SETPARAFORMAT 0x0447
!endif
; In Arabic, NSIS gives the licence box WS_EX_RTLREADING and WS_EX_LEFTSCROLLBAR. The RichEdit lays
; its lines out as if the scroll bar were on the right, so every line ran past the right edge and
; lost its first letters, and the English text was right to left too. Instead, the box drops those
; two styles and sets each paragraph's direction itself (PARAFORMAT, 156 bytes, dwMask PFM_RTLPARA):
; the Arabic text (the first WASL_LICENCE_AR_CHARS characters) right to left, the rest left to right.
Function LicenceShow
  ${If} $LANGUAGE = ${LANG_ARABIC}
    FindWindow $0 "#32770" "" $HWNDPARENT
    GetDlgItem $0 $0 1000
    System::Call "user32::GetWindowLongW(p r0, i ${GWL_EXSTYLE}) i .r1"
    IntOp $1 $1 & 0xFFFF9FFF ; clear WS_EX_LEFTSCROLLBAR (0x4000) and WS_EX_RTLREADING (0x2000)
    System::Call "user32::SetWindowLongW(p r0, i ${GWL_EXSTYLE}, i r1)"
    ; SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED
    System::Call "user32::SetWindowPos(p r0, p 0, i 0, i 0, i 0, i 0, i 0x27)"
    System::Alloc 156
    Pop $1
    System::Call "*$1(i 156, i 0x10000, &i2 0, &i2 0)"
    SendMessage $0 ${EM_SETSEL} 0 -1
    SendMessage $0 ${EM_SETPARAFORMAT} 0 $1
    System::Call "*$1(i 156, i 0x10000, &i2 0, &i2 1)"
    SendMessage $0 ${EM_SETSEL} 0 ${WASL_LICENCE_AR_CHARS}
    SendMessage $0 ${EM_SETPARAFORMAT} 0 $1
    SendMessage $0 ${EM_SETSEL} 0 0
    System::Free $1
  ${EndIf}
FunctionEnd
!endif

; ---------------------------------------------------------------- helpers
; Runs wasl-installer.ps1 (extracted to $PLUGINSDIR) hidden; its exit code is left in $0.
!macro WaslHelper ARGS
  nsExec::ExecToLog '"${POWERSHELL}" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\wasl-installer.ps1" ${ARGS}'
  Pop $0
!macroend
; The install folder for the helper. "\." keeps a drive root (D:\) from ending in \", which the
; command line would read as an escaped quote.
!define WASLDIR '-Dir "$INSTDIR\."'

; Stops this install's wasld and app (and, with -MsiProduct, then removes an old .msi); the helper's
; answer is left in $WasRunning. Tauri's check then catches anything still holding the files.
!macro WaslStopInDir EXTRA
  DetailPrint "$(wStopping)"
  !insertmacro WaslHelper 'stop-in-dir ${WASLDIR} ${EXTRA}'
  StrCpy $WasRunning $0
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro CheckIfAppIsRunning "$INSTDIR\wasld.exe" "wasld"
  !insertmacro CheckIfAppIsRunning "$INSTDIR\wasl.exe" "wasl"
!macroend

; path-add or path-remove for this install's scope, then tells running programs. Exit code in $0.
!macro WaslPath CMD
  !insertmacro WaslPathScope $1
  !insertmacro WaslHelper '${CMD} -Scope $1 ${WASLDIR}'
  !insertmacro WaslBroadcastEnvironment
!macroend

!macro WaslExtractHelper
  InitPluginsDir
  File "/oname=$PLUGINSDIR\wasl-installer.ps1" "${WASL_NSIS_DIR}\wasl-installer.ps1"
!macroend

; Tells Explorer and running programs that PATH changed, so a new terminal picks it up at once.
!macro WaslBroadcastEnvironment
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro WaslPathScope OUT
  ${If} $Scope == "all"
    StrCpy ${OUT} "Machine"
  ${Else}
    StrCpy ${OUT} "User"
  ${EndIf}
!macroend

; Prints to the console a silent install was started from.
!macro WaslConsoleError MSG
  System::Call 'kernel32::AttachConsole(i -1)i.r0'
  ${If} $0 <> 0
    System::Call 'kernel32::GetStdHandle(i -11)i.r0'
    System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)'
    FileWrite $0 "${MSG}"
  ${EndIf}
!macroend

; Shell context and registry view for $Scope.
!macro WaslApplyScope
  ${If} $Scope == "all"
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}
  ${If} ${RunningX64}
    !if "${ARCH}" == "x86"
      SetRegView 32
    !else
      SetRegView 64
    !endif
  ${EndIf}
!macroend

; Which hive holds an install of Wasl: "user", "all" or "".
!macro WaslFindExisting OUT
  ; The 64-bit view: this installer is a 32-bit program, and 0.4.0 wrote its keys there.
  ${If} ${RunningX64}
    !if "${ARCH}" != "x86"
      SetRegView 64
    !endif
  ${EndIf}
  StrCpy ${OUT} ""
  ReadRegStr $1 HKCU "${UNINSTKEY}" "UninstallString"
  ${If} $1 != ""
    StrCpy ${OUT} "user"
  ${Else}
    ReadRegStr $1 HKLM "${UNINSTKEY}" "UninstallString"
    ${IfThen} $1 != "" ${|} StrCpy ${OUT} "all" ${|}
  ${EndIf}
!macroend

!macro WaslIsAdmin OUT
  UserInfo::GetAccountType
  Pop ${OUT}
!macroend

; $OtherUser = 1 when an administrator other than the person at the desktop elevated this; the
; helper then acts on the desktop user's sign-in entry and processes, and the data is kept.
!macro WaslCheckOtherUser
  StrCpy $OtherUser 0
  !insertmacro WaslIsAdmin $0
  ${If} $0 == "Admin"
    !insertmacro WaslHelper 'is-desktop-user'
    ${IfThen} $0 = 1 ${|} StrCpy $OtherUser 1 ${|}
  ${EndIf}
!macroend

; ---------------------------------------------------------------- sections
Section "-WebView2"
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}

  ${If} $4 == ""
    ${If} $UpdateMode <> 1
      !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        DetailPrint "$(webview2Downloading)"
        NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Pop $0
        ${If} $0 == "success"
          DetailPrint "$(webview2DownloadSuccess)"
        ${Else}
          DetailPrint "$(webview2DownloadError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif
      !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif
      !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
        Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        Goto install_webview2
      !endif
      Goto webview2_done
      install_webview2:
        DetailPrint "$(installingWebview2)"
        ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
        ${If} $1 = 0
          DetailPrint "$(webview2InstallSuccess)"
        ${Else}
          DetailPrint "$(webview2InstallError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
      webview2_done:
    ${EndIf}
  ${Else}
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      ${VersionCompare} "${MINIMUMWEBVIEW2VERSION}" "$4" $R0
      ${If} $R0 = 1
        update_webview:
          DetailPrint "$(installingWebview2)"
          ${If} ${RunningX64}
            ReadRegStr $R1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate" "path"
          ${Else}
            ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 == ""
            ReadRegStr $R1 HKCU "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 != ""
            ExecWait `"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1
            ${If} $1 = 0
              DetailPrint "$(webview2InstallSuccess)"
            ${Else}
              MessageBox MB_ICONEXCLAMATION|MB_ABORTRETRYIGNORE "$(webview2InstallError)" IDIGNORE ignore IDRETRY update_webview
              Quit
              ignore:
            ${EndIf}
          ${EndIf}
      ${EndIf}
    !endif
  ${EndIf}
SectionEnd

Section "!$(wSecCore)" SEC_CORE
  SectionIn RO
  SetOutPath $INSTDIR
  !insertmacro WaslExtractHelper

  !ifmacrodef NSIS_HOOK_PREINSTALL
    !insertmacro NSIS_HOOK_PREINSTALL
  !endif

  ; Upgrade in place: stop this install's wasld (asked politely first) and tray app, and remember
  ; which ran so they start again at the end. A Wasl running from another folder is left alone.
  ; Nothing is removed before this point, the person's last click on Install: an old .msi (which
  ; can share this folder, as 0.3.3 does with 0.4.0) goes only after its wasld has stopped.
  ; Silent installs skip the reinstall page, where this is found otherwise.
  ${IfThen} ${Silent} ${|} Call FindWixInstall ${|}
  StrCpy $R9 ""
  ${If} $WixMode = 1
    StrCpy $R9 '-MsiProduct "$WixProduct"'
    ${IfThen} ${Silent} ${|} StrCpy $R9 "$R9 -Quiet" ${|}
  ${EndIf}
  !insertmacro WaslStopInDir '$R9'
  ; 8: the .msi could not be removed; 64 and up: the helper itself failed.
  ${If} $WasRunning >= 64
    StrCpy $1 1
    StrCpy $WasRunning 0
  ${Else}
    IntOp $1 $WasRunning & 8
    IntOp $WasRunning $WasRunning & 3
  ${EndIf}
  ${If} $WixMode = 1
  ${AndIf} $1 <> 0
    MessageBox MB_ICONEXCLAMATION "$(wMsiFailed)" /SD IDOK
    Call RestartStopped
    Abort "$(wMsiFailed)"
  ${EndIf}

  ${If} $RemoveOld = 1
    Call RemoveOldInstall
  ${EndIf}

  File "${MAINBINARYSRCPATH}"

  {{#each resources_dirs}}
    CreateDirectory "$INSTDIR\\{{this}}"
  {{/each}}
  {{#each resources}}
    File /a "/oname={{this.[1]}}" "{{no-escape @key}}"
  {{/each}}

  {{#each binaries}}
    File /a "/oname={{this}}" "{{no-escape @key}}"
  {{/each}}

  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
       !insertmacro APP_ASSOCIATE "{{ext}}" "{{or association.name ext}}" "{{association-description association.description ext}}" "$INSTDIR\${MAINBINARYNAME}.exe,0" "Open with ${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe $\"%1$\""
    {{/each}}
  {{/each}}

  {{#each deep_link_protocols as |protocol| ~}}
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "URL Protocol" ""
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "" "URL:${BUNDLEID} protocol"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  {{/each}}

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR

  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $OldMainBinaryName != ""
  ${AndIf} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
    Delete "$INSTDIR\$OldMainBinaryName"
  ${EndIf}
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"

  ${If} $Scope == "all"
    StrCpy $1 "/ALLUSERS"
  ${Else}
    StrCpy $1 "/CURRENTUSER"
  ${EndIf}
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\" $1"
  WriteRegStr SHCTX "${UNINSTKEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" $1 /S"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"

  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $0 $1 $2
  IntOp $0 $0 + ${ESTIMATEDSIZE}
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"

  !if "${HOMEPAGE}" != ""
    WriteRegStr SHCTX "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "URLUpdateInfo" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}"
  !endif
SectionEnd

Section "$(wSecCli)" SEC_CLI
  DetailPrint "$(wPathAdd)"
  !insertmacro WaslPath path-add
  ; PathAdded: the entry is this installer's, so uninstalling may take it away. An entry that was
  ; already there (10) stays the person's, unless an earlier install recorded adding it.
  ${If} $0 = 0
    WriteRegDWORD SHCTX "${MANUPRODUCTKEY}" "PathAdded" 1
  ${ElseIf} $0 <> 10
    DetailPrint "$(wPathFailed)"
  ${EndIf}
SectionEnd

Section "$(wSecStartMenu)" SEC_STARTMENU
  Call CreateOrUpdateStartMenuShortcut
SectionEnd

Section /o "$(wSecDesktop)" SEC_DESKTOP
  Call CreateOrUpdateDesktopShortcut
SectionEnd

Section "$(wSecAutostart)" SEC_AUTOSTART
  DetailPrint "$(wAutostartOn)"
  ; The same entry Wasl's "Start at login" setting writes (wasl-cli service.rs). It lives in the
  ; signed-in person's HKCU, so an administrator over the shoulder starts it as that person.
  ${If} $OtherUser = 1
    nsis_tauri_utils::RunAsUser "$INSTDIR\wasl.exe" "service install"
  ${Else}
    nsExec::ExecToLog '"$INSTDIR\wasl.exe" service install'
    Pop $0
  ${EndIf}
SectionEnd

Section "-Post"
  ; Components turned off on an upgrade: undo what an earlier install did, and remember the choices.
  ${IfNot} ${SectionIsSelected} ${SEC_CLI}
    ReadRegDWORD $1 SHCTX "${MANUPRODUCTKEY}" "PathAdded"
    ${If} $1 = 1
      DetailPrint "$(wPathRemove)"
      !insertmacro WaslPath path-remove
      DeleteRegValue SHCTX "${MANUPRODUCTKEY}" "PathAdded"
    ${EndIf}
  ${EndIf}
  ${IfNot} ${SectionIsSelected} ${SEC_STARTMENU}
  ${AndIf} $UpdateMode <> 1
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${IfThen} $0 = 1 ${|} Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk" ${|}
  ${EndIf}
  ${IfNot} ${SectionIsSelected} ${SEC_DESKTOP}
  ${AndIf} $UpdateMode <> 1
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${IfThen} $0 = 1 ${|} Delete "$DESKTOP\${PRODUCTNAME}.lnk" ${|}
  ${EndIf}
  ${IfNot} ${SectionIsSelected} ${SEC_AUTOSTART}
  ${AndIf} $UpdateMode <> 1
    !insertmacro WaslHelper 'run-entry-remove ${WASLDIR}'
  ${EndIf}

  !macro WaslRememberSection SEC NAME
    ${If} ${SectionIsSelected} ${SEC}
      WriteRegDWORD SHCTX "${MANUPRODUCTKEY}" "${NAME}" 1
    ${Else}
      WriteRegDWORD SHCTX "${MANUPRODUCTKEY}" "${NAME}" 0
    ${EndIf}
  !macroend
  !insertmacro WaslRememberSection ${SEC_CLI} "CliOnPath"
  !insertmacro WaslRememberSection ${SEC_STARTMENU} "StartMenuShortcut"
  !insertmacro WaslRememberSection ${SEC_DESKTOP} "DesktopShortcut"
  !insertmacro WaslRememberSection ${SEC_AUTOSTART} "StartAtSignIn"

  Call RestartStopped

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CORE} "$(wSecCoreDesc)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CLI} "$CliDesc"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_STARTMENU} "$(wSecStartMenuDesc)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "$(wSecDesktopDesc)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_AUTOSTART} "$(wSecAutostartDesc)"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section Uninstall
  !ifmacrodef NSIS_HOOK_PREUNINSTALL
    !insertmacro NSIS_HOOK_PREUNINSTALL
  !endif

  !insertmacro WaslExtractHelper
  !insertmacro WaslStopInDir ''

  ${If} $UpdateMode <> 1
    ; The PATH entry only when this installer added it; one that was there before stays.
    ReadRegDWORD $1 SHCTX "${MANUPRODUCTKEY}" "PathAdded"
    ${If} $1 = 1
      DetailPrint "$(wPathRemove)"
      !insertmacro WaslPath path-remove
      DeleteRegValue SHCTX "${MANUPRODUCTKEY}" "PathAdded"
    ${EndIf}
    ; Only this install's "start at sign-in" entry; another copy's stays.
    !insertmacro WaslHelper 'run-entry-remove ${WASLDIR}'
  ${EndIf}

  Delete "$INSTDIR\${MAINBINARYNAME}.exe"
  {{#each resources}}
    Delete "$INSTDIR\\{{this.[1]}}"
  {{/each}}
  {{#each binaries}}
    Delete "$INSTDIR\\{{this}}"
  {{/each}}

  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
      !insertmacro APP_UNASSOCIATE "{{ext}}" "{{or association.name ext}}"
    {{/each}}
  {{/each}}

  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}

  Delete "$INSTDIR\uninstall.exe"
  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  ${If} $UpdateMode <> 1
    ${If} $DataShared <> 1
      !insertmacro DeleteAppUserModelId
    ${EndIf}
    ; Shortcuts are removed only when they point at this install. 0.4.0 and earlier could also
    ; have put one in a Start menu folder; those are handled the same way.
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk"
      RMDir "$SMPROGRAMS\${PRODUCTNAME}"
    ${EndIf}
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  DeleteRegKey SHCTX "${UNINSTKEY}"

  ; Wasl's data is kept unless the person ticked the box (or passed /DELETEDATA), and always when
  ; another copy of Wasl on this computer still uses it or when another account elevated this
  ; uninstaller ($LOCALAPPDATA would be that account's, not the person's).
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
  ${AndIf} $DataShared <> 1
  ${AndIf} $OtherUser <> 1
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"

    SetShellVarContext current
    ${If} "${WASLDATADIR}" != "$INSTDIR"
      RmDir /r "${WASLDATADIR}"
    ${EndIf}
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
    !insertmacro NSIS_HOOK_POSTUNINSTALL
  !endif

  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

; ---------------------------------------------------------------- installer functions
Function .onInit
  ${GetOptions} $CMDLINE "/P" $0
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/NS" $0
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/ELEVATED" $0
  ${IfNot} ${Errors}
    StrCpy $Elevated 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/LANG=" $0
  ${IfNot} ${Errors}
    StrCpy $LANGUAGE $0
  ${Else}
    !if "${DISPLAYLANGUAGESELECTOR}" == "true"
      ${If} $PassiveMode <> 1
        !insertmacro MUI_LANGDLL_DISPLAY
      ${EndIf}
    !endif
  ${EndIf}

  ; Scope: the command line, else where Wasl is already installed, else just me.
  !insertmacro WaslFindExisting $ExistingScope
  StrCpy $Scope "user"
  ${IfThen} $ExistingScope == "all" ${|} StrCpy $Scope "all" ${|}
  ${GetOptions} $CMDLINE "/ALLUSERS" $0
  ${IfNot} ${Errors}
    StrCpy $Scope "all"
    StrCpy $ScopeLocked 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/CURRENTUSER" $0
  ${IfNot} ${Errors}
    StrCpy $Scope "user"
    StrCpy $ScopeLocked 1
  ${EndIf}

  ${If} $Scope == "all"
  ${AndIf} ${Silent}
    !insertmacro WaslIsAdmin $0
    ${If} $0 != "Admin"
      !insertmacro WaslConsoleError "$(wNeedsAdmin)"
      SetErrorLevel ${ERROR_ELEVATION_REQUIRED}
      Quit
    ${EndIf}
  ${EndIf}

  !insertmacro WaslExtractHelper
  !insertmacro WaslCheckOtherUser
  Call ApplyScope
  Call ApplyComponents

  !if "${ALLOWDOWNGRADES}" == "false"
  ${If} ${Silent}
  ${OrIf} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    ReadRegStr $R0 SHCTX "${UNINSTKEY}" "DisplayVersion"
    nsis_tauri_utils::SemverCompare "${VERSION}" $R0
    Pop $R0
    ${If} $R0 = -1
      ${If} ${Silent}
        !insertmacro WaslConsoleError "$(silentDowngrades)"
      ${Else}
        MessageBox MB_ICONSTOP "$(silentDowngrades)"
      ${EndIf}
      Abort
    ${EndIf}
  ${EndIf}
  !endif
FunctionEnd

; Components: the defaults; then what was chosen last time or, from a build that stored no choice
; (0.4.0), what that install left in place; then the command line. Runs again when the scope changes.
Function ApplyComponents
  !insertmacro SelectSection ${SEC_CLI}
  !insertmacro SelectSection ${SEC_STARTMENU}
  !insertmacro UnselectSection ${SEC_DESKTOP}
  !insertmacro SelectSection ${SEC_AUTOSTART}

  ReadRegDWORD $0 SHCTX "${MANUPRODUCTKEY}" "CliOnPath"
  ${If} $0 == "0"
    !insertmacro UnselectSection ${SEC_CLI}
  ${EndIf}
  ReadRegDWORD $0 SHCTX "${MANUPRODUCTKEY}" "StartMenuShortcut"
  ${If} $0 == "0"
    !insertmacro UnselectSection ${SEC_STARTMENU}
  ${EndIf}

  ; $3: an install is registered in this scope; $2: what it left (1 desktop shortcut, 2 sign-in entry).
  StrCpy $2 0
  ReadRegStr $3 SHCTX "${UNINSTKEY}" "UninstallString"
  ${If} $3 != ""
    !insertmacro WaslHelper 'component-state ${WASLDIR} -Link "$DESKTOP\${PRODUCTNAME}.lnk"'
    ${IfThen} $0 <= 3 ${|} StrCpy $2 $0 ${|}
  ${EndIf}
  ClearErrors
  ReadRegDWORD $0 SHCTX "${MANUPRODUCTKEY}" "DesktopShortcut"
  ${If} ${Errors}
    IntOp $0 $2 & 1
  ${EndIf}
  ${If} $0 = 1
    !insertmacro SelectSection ${SEC_DESKTOP}
  ${EndIf}
  ClearErrors
  ReadRegDWORD $0 SHCTX "${MANUPRODUCTKEY}" "StartAtSignIn"
  ${If} ${Errors}
    ${If} $3 != ""
      IntOp $0 $2 & 2
      ${IfThen} $0 = 0 ${|} !insertmacro UnselectSection ${SEC_AUTOSTART} ${|}
    ${EndIf}
  ${ElseIf} $0 = 0
    !insertmacro UnselectSection ${SEC_AUTOSTART}
  ${EndIf}

  ${GetOptions} $CMDLINE "/NOCLI" $0
  ${IfNot} ${Errors}
    !insertmacro UnselectSection ${SEC_CLI}
  ${EndIf}
  ${GetOptions} $CMDLINE "/NOSTARTMENU" $0
  ${IfNot} ${Errors}
    !insertmacro UnselectSection ${SEC_STARTMENU}
  ${EndIf}
  ${GetOptions} $CMDLINE "/DESKTOP" $0
  ${IfNot} ${Errors}
    !insertmacro SelectSection ${SEC_DESKTOP}
  ${EndIf}
  ${GetOptions} $CMDLINE "/NOAUTOSTART" $0
  ${IfNot} ${Errors}
    !insertmacro UnselectSection ${SEC_AUTOSTART}
  ${EndIf}
  ${If} $NoShortcutMode = 1
    !insertmacro UnselectSection ${SEC_STARTMENU}
    !insertmacro UnselectSection ${SEC_DESKTOP}
  ${EndIf}
FunctionEnd

; Sets the shell context for $Scope and, unless /D= gave a folder, the default or previous folder.
Var DirGiven
Function ApplyScope
  !insertmacro WaslApplyScope
  ${If} $Scope == "all"
    StrCpy $CliDesc "$(wSecCliDescAll)"
  ${Else}
    StrCpy $CliDesc "$(wSecCliDescUser)"
  ${EndIf}
  ${If} $DirGiven == ""
    ${If} $INSTDIR != "${PLACEHOLDER_INSTALL_DIR}"
      StrCpy $DirGiven 1
    ${Else}
      StrCpy $DirGiven 0
    ${EndIf}
  ${EndIf}
  ${If} $DirGiven = 1
    Return
  ${EndIf}
  ${If} $Scope == "all"
    !if "${ARCH}" == "x86"
      StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
    !else
      StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
    !endif
  ${Else}
    ; Not %LOCALAPPDATA%\Wasl: that is wasld's data folder.
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
  ${EndIf}
  ; The previous folder, only while that install is still registered (a stale value can name
  ; %LOCALAPPDATA%\Wasl, where old per-user builds went and wasld keeps its data).
  ReadRegStr $5 SHCTX "${UNINSTKEY}" "UninstallString"
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  ${If} $4 != ""
  ${AndIf} $5 != ""
    StrCpy $INSTDIR $4
  ${EndIf}
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
FunctionEnd

Function SkipIfPassiveOrElevated
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
  ${IfThen} $Elevated = 1 ${|} Abort ${|}
FunctionEnd

Var ScopeUserRadio
Var ScopeAllRadio
Function ScopePage
  ${If} $PassiveMode = 1
  ${OrIf} $ScopeLocked = 1
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "$(wScopeTitle)" "$(wScopeSub)"
  nsDialogs::Create 1018
  Pop $0
  ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}

  ${NSD_CreateRadioButton} 0 6u 100% 12u "$(wScopeUser)"
  Pop $ScopeUserRadio
  ${NSD_CreateLabel} 14u 20u -14u 18u "$(wScopeUserNote)"
  Pop $0
  ${NSD_CreateRadioButton} 0 46u 100% 12u "$(wScopeAll)"
  Pop $ScopeAllRadio
  ${NSD_CreateLabel} 14u 60u -14u 26u "$(wScopeAllNote)"
  Pop $0

  ${If} $ExistingScope != ""
    ${If} $ExistingScope == "all"
      StrCpy $0 "$(wScopeFoundAll)"
    ${Else}
      StrCpy $0 "$(wScopeFoundUser)"
    ${EndIf}
    ${NSD_CreateLabel} 0 100u 100% 24u "$(wScopeFound)"
    Pop $0
  ${EndIf}

  ${If} $Scope == "all"
    ${NSD_Check} $ScopeAllRadio
    ${NSD_SetFocus} $ScopeAllRadio
  ${Else}
    ${NSD_Check} $ScopeUserRadio
    ${NSD_SetFocus} $ScopeUserRadio
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function ScopeLeave
  ${NSD_GetState} $ScopeAllRadio $0
  ${If} $0 == ${BST_CHECKED}
    !insertmacro WaslIsAdmin $1
    ${If} $1 != "Admin"
      ; Ask for administrator rights by starting this installer again, elevated, past these pages.
      System::Call 'shell32::ShellExecuteW(p $HWNDPARENT, w "runas", w "$EXEPATH", w "/ALLUSERS /ELEVATED /LANG=$LANGUAGE", p 0, i ${SW_SHOWNORMAL}) p .r2'
      ${If} $2 > 32
        Quit
      ${EndIf}
      MessageBox MB_ICONEXCLAMATION "$(wElevateFailed)"
      Abort
    ${EndIf}
    StrCpy $Scope "all"
  ${Else}
    StrCpy $Scope "user"
  ${EndIf}
  Call ApplyScope
  Call ApplyComponents
FunctionEnd

; A WiX (.msi) install of Wasl (0.3.x), which is replaced as Tauri's template does. Those were
; all-users installs, so only an all-users install looks for one; "just me" leaves it alone. Every
; entry is checked: with both installed, the NSIS entry "Wasl" sorts before the .msi's {GUID}.
; Only finds it: SEC_CORE removes it, after Install is clicked and its wasld is stopped.
Function FindWixInstall
  StrCpy $WixMode 0
  StrCpy $WixProduct ""
  ${If} $Scope != "all"
    Return
  ${EndIf}
  StrCpy $0 0
  wix_loop:
    EnumRegKey $1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall" $0
    StrCmp $1 "" wix_done
    IntOp $0 $0 + 1
    !insertmacro WaslIsWixEntry HKLM SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1 "${PRODUCTNAME}" "${MANUFACTURER}" $R2
    StrCmp $R2 1 0 wix_loop
    StrCpy $WixMode 1
    StrCpy $WixProduct $1
    StrCpy $R6 "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1"
  wix_done:
FunctionEnd

Function PageReinstall
  StrCpy $RemoveOld 0
  Call FindWixInstall
  ${If} $WixMode = 1
    Goto compare_version
  ${EndIf}

  ReadRegStr $R0 SHCTX "${UNINSTKEY}" ""
  ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
  ${IfThen} "$R0$R1" == "" ${|} Abort ${|}

  compare_version:
  ${If} $WixMode = 1
    ReadRegStr $R0 HKLM "$R6" "DisplayVersion"
  ${Else}
    ReadRegStr $R0 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${EndIf}
  StrCpy $R4 $R0
  ${IfThen} $R4 == "" ${|} StrCpy $R4 "$(unknown)" ${|}

  nsis_tauri_utils::SemverCompare "${VERSION}" $R0
  Pop $R0
  ${If} $R0 = 0
    StrCpy $R1 "$(alreadyInstalledLong)"
    StrCpy $R2 "$(addOrReinstall)"
    StrCpy $R3 "$(uninstallApp)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(chooseMaintenanceOption)"
  ${ElseIf} $R0 = 1
    ; Upgrading: in place by default, so the settings, pairs and history stay where they are.
    StrCpy $R1 "$(wUpgradeLong)"
    StrCpy $R2 "$(wUpgradeInPlace)"
    StrCpy $R3 "$(wUninstallFirst)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
  ${ElseIf} $R0 = -1
    StrCpy $R1 "$(newerVersionInstalled)"
    StrCpy $R2 "$(uninstallBeforeInstalling)"
    !if "${ALLOWDOWNGRADES}" == "true"
      StrCpy $R3 "$(dontUninstall)"
    !else
      StrCpy $R3 "$(dontUninstallDowngrade)"
    !endif
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
  ${Else}
    Abort
  ${EndIf}

  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    Call PageLeaveReinstall
  ${Else}
    nsDialogs::Create 1018
    Pop $R4
    ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}
    ${NSD_CreateLabel} 0 0 100% 36u $R1
    Pop $R1
    ${NSD_CreateRadioButton} 30u 50u -30u 12u $R2
    Pop $R2
    ${NSD_OnClick} $R2 PageReinstallUpdateSelection
    ${NSD_CreateRadioButton} 30u 70u -30u 12u $R3
    Pop $R3
    !if "${ALLOWDOWNGRADES}" == "false"
      ${IfThen} $R0 = -1 ${|} EnableWindow $R3 0 ${|}
    !endif
    ${NSD_OnClick} $R3 PageReinstallUpdateSelection
    ${If} $ReinstallPageCheck <> 2
      SendMessage $R2 ${BM_SETCHECK} ${BST_CHECKED} 0
    ${Else}
      SendMessage $R3 ${BM_SETCHECK} ${BST_CHECKED} 0
    ${EndIf}
    ${NSD_SetFocus} $R2
    nsDialogs::Show
  ${EndIf}
FunctionEnd

Function PageReinstallUpdateSelection
  ${NSD_GetState} $R2 $R1
  ${If} $R1 == ${BST_CHECKED}
    StrCpy $ReinstallPageCheck 1
  ${Else}
    StrCpy $ReinstallPageCheck 2
  ${EndIf}
FunctionEnd

Function PageLeaveReinstall
  ${NSD_GetState} $R2 $R1
  ; Passive and silent installs take the first choice.
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    StrCpy $R1 1
  ${EndIf}

  ; Only records the choice: anything is removed in SEC_CORE, once Install is clicked.
  StrCpy $RemoveOld 0
  ${If} $WixMode = 1
  ${OrIf} $UpdateMode = 1
    Return
  ${EndIf}

  ; $R0: same version (0), upgrading (1), downgrading (-1). $R1 = 1: the first choice.
  ${If} $R0 = -1
    ${IfThen} $R1 = 1 ${|} StrCpy $RemoveOld 1 ${|}
  ${ElseIf} $R1 <> 1
    StrCpy $RemoveOld 1
  ${EndIf}
FunctionEnd

; "Uninstall the old version first", run from SEC_CORE after this folder's Wasl was stopped. The old
; uninstaller runs silently, so it keeps the data, as the choice says.
Function RemoveOldInstall
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
  ${If} $R1 == ""
    Return
  ${EndIf}
  ${If} $4 != ""
  ${AndIf} $4 != $INSTDIR
    !insertmacro WaslHelper 'stop-in-dir -Dir "$4\."'
  ${EndIf}
  DetailPrint "$(wRemovingOld)"
  StrCpy $R1 "$R1 /S"
  ${IfThen} $UpdateMode = 1 ${|} StrCpy $R1 "$R1 /UPDATE" ${|}
  ${IfThen} $4 != "" ${|} StrCpy $R1 "$R1 _?=$4" ${|}
  ClearErrors
  ExecWait '$R1' $0
  ${IfThen} ${Errors} ${|} StrCpy $0 2 ${|}
  ${If} $0 <> 0
  ${OrIf} ${FileExists} "$4\${MAINBINARYNAME}.exe"
    MessageBox MB_ICONEXCLAMATION "$(unableToUninstall)" /SD IDOK
    Call RestartStopped
    Abort "$(unableToUninstall)"
  ${EndIf}
  ${If} $4 != ""
    Delete "$4\uninstall.exe"
    RMDir "$4"
  ${EndIf}
FunctionEnd

; Starts again what stop-in-dir stopped, as the signed-in user even when this installer is elevated.
Function RestartStopped
  IntOp $1 $WasRunning & 1
  ${If} $1 <> 0
    DetailPrint "$(wRestarting)"
    nsis_tauri_utils::RunAsUser "$INSTDIR\wasld.exe" ""
  ${EndIf}
  IntOp $1 $WasRunning & 2
  ${If} $1 <> 0
    nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "--background"
  ${EndIf}
FunctionEnd

Function .onInstSuccess
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function FinishPre
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
  ${If} ${SectionIsSelected} ${SEC_CLI}
    StrCpy $FinishText "$(wFinishTextCli)"
  ${Else}
    StrCpy $FinishText "$(wFinishText)"
  ${EndIf}
FunctionEnd

Function FinishShow
  ${IfNot} ${SectionIsSelected} ${SEC_CLI}
    ShowWindow $mui.FinishPage.ShowReadme ${SW_HIDE}
  ${EndIf}
  ; The upgrade already started the app again.
  IntOp $0 $WasRunning & 2
  ${If} $0 <> 0
    SendMessage $mui.FinishPage.Run ${BM_SETCHECK} ${BST_UNCHECKED} 0
  ${EndIf}
FunctionEnd

Function RunMainBinary
  nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
FunctionEnd

; A new PowerShell window that reads PATH afresh, as any terminal opened from now on would.
Function TryCli
  nsis_tauri_utils::RunAsUser "${POWERSHELL}" `-NoExit -NoLogo -Command "$$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User'); wasl status"`
FunctionEnd

Function CreateOrUpdateStartMenuShortcut
  ; Tauri's migration: a shortcut to an older main binary name is pointed at the new one.
  !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Return
  ${EndIf}
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}
  CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
FunctionEnd

Function CreateOrUpdateDesktopShortcut
  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Return
  ${EndIf}
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}
  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd

; ---------------------------------------------------------------- uninstaller functions
Function un.onInit
  ${GetOptions} $CMDLINE "/P" $0
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/DELETEDATA" $0
  ${IfNot} ${Errors}
    StrCpy $DeleteAppDataCheckboxState 1
  ${EndIf}

  ; Language first, so a message below is in the person's language.
  !insertmacro MUI_UNGETLANGUAGE
  ${GetOptions} $CMDLINE "/LANG=" $0
  ${IfNot} ${Errors}
    StrCpy $LANGUAGE $0
  ${EndIf}

  ; The scope this copy was installed with: the one whose uninstall entry starts this uninstaller,
  ; so uninstall.exe run from its folder with no switch works for both. /ALLUSERS and /CURRENTUSER
  ; (the uninstall entries pass one) say it outright.
  !insertmacro WaslFindExisting $0
  StrCpy $Scope "user"
  StrCpy $2 '"$INSTDIR\uninstall.exe"'
  StrLen $3 $2
  ReadRegStr $1 HKLM "${UNINSTKEY}" "UninstallString"
  StrCpy $1 $1 $3
  ${IfThen} $1 == $2 ${|} StrCpy $Scope "all" ${|}
  ReadRegStr $1 HKCU "${UNINSTKEY}" "UninstallString"
  StrCpy $1 $1 $3
  ${IfThen} $1 == $2 ${|} StrCpy $Scope "user" ${|}
  ${GetOptions} $CMDLINE "/ALLUSERS" $0
  ${IfNot} ${Errors}
    StrCpy $Scope "all"
  ${EndIf}
  ${GetOptions} $CMDLINE "/CURRENTUSER" $0
  ${IfNot} ${Errors}
    StrCpy $Scope "user"
  ${EndIf}
  !insertmacro WaslApplyScope

  ${If} $Scope == "all"
    !insertmacro WaslIsAdmin $0
    ${If} $0 != "Admin"
      ${If} ${Silent}
        !insertmacro WaslConsoleError "$(wNeedsAdmin)"
        SetErrorLevel ${ERROR_ELEVATION_REQUIRED}
        Quit
      ${EndIf}
      StrCpy $1 "/ALLUSERS /LANG=$LANGUAGE"
      ${IfThen} $DeleteAppDataCheckboxState = 1 ${|} StrCpy $1 "$1 /DELETEDATA" ${|}
      ${IfThen} $PassiveMode = 1 ${|} StrCpy $1 "$1 /P" ${|}
      StrCpy $1 "$1 _?=$INSTDIR"
      System::Call 'shell32::ShellExecuteW(p 0, w "runas", w "$EXEPATH", w "$1", p 0, i ${SW_SHOWNORMAL}) p .r2'
      ${If} $2 > 32
        Quit
      ${EndIf}
      ; Declined or failed: nothing was removed.
      MessageBox MB_ICONEXCLAMATION "$(wUnElevateFailed)"
      SetErrorLevel ${ERROR_ELEVATION_REQUIRED}
      Quit
    ${EndIf}
  ${EndIf}

  !insertmacro WaslExtractHelper
  !insertmacro WaslCheckOtherUser

  ; The data is per person and shared by every copy of Wasl they have: keep it while another
  ; copy (the other scope) is still installed.
  StrCpy $DataShared 0
  ${If} $Scope == "all"
    ReadRegStr $1 HKCU "${UNINSTKEY}" "UninstallString"
  ${Else}
    ReadRegStr $1 HKLM "${UNINSTKEY}" "UninstallString"
  ${EndIf}
  ${IfThen} $1 != "" ${|} StrCpy $DataShared 1 ${|}
FunctionEnd

Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
FunctionEnd

Function un.ConfirmShow
  FindWindow $1 "#32770" "" $HWNDPARENT
  System::Call "user32::GetDpiForWindow(p r1) i .r2"
  ${If} $(^RTL) = 1
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE} | ${WS_EX_LAYOUTRTL}"
    IntOp $4 50 * $2
  ${Else}
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE}"
    IntOp $4 0 * $2
  ${EndIf}
  IntOp $5 100 * $2
  IntOp $6 420 * $2
  IntOp $7 36 * $2
  IntOp $4 $4 / 96
  IntOp $5 $5 / 96
  IntOp $6 $6 / 96
  IntOp $7 $7 / 96
  ${If} $DataShared = 1
    StrCpy $0 "$(wKeepDataShared)"
  ${ElseIf} $OtherUser = 1
    StrCpy $0 "$(wKeepDataOtherUser)"
  ${Else}
    StrCpy $0 "$(wDeleteData)"
  ${EndIf}
  System::Call 'user32::CreateWindowEx(i r3, w "${__NSD_CheckBox_CLASS}", w r0, i ${__NSD_CheckBox_STYLE}|${BS_MULTILINE}, i r4, i r5, i r6, i r7, p r1, i0, i0, i0) i .s'
  Pop $DeleteAppDataCheckbox
  SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $1
  SendMessage $DeleteAppDataCheckbox ${WM_SETFONT} $1 1
  ${If} $DataShared = 1
  ${OrIf} $OtherUser = 1
    EnableWindow $DeleteAppDataCheckbox 0
  ${EndIf}
FunctionEnd

Function un.ConfirmLeave
  SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
FunctionEnd
