; kimchi's Windows installer (scripts/bundle-windows.sh runs makensis on it):
;   makensis /DVERSION=0.2.0 /DSRC=<folder with kimchi.exe…> /DOUTFILE=<setup.exe> installer.nsi
;
; Per-user install into %LOCALAPPDATA%\kimchi with the same uninstall key and Start menu shortcut
; as the Tauri builds (0.1.x), so installing over them replaces them in place. kimchi 0.1.x's
; updater runs this installer with `/P /R /UPDATE /ARGS …` (passive, restart): /P skips the pages,
; /R starts kimchi when done; /S is NSIS's silent mode.

Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define PRODUCT "kimchi"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}"

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\${PRODUCT}"
InstallDirRegKey HKCU "${UNINSTKEY}" "InstallLocation"
BrandingText "kimchi ${VERSION} · lsuite"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "kimchi installer"
VIAddVersionKey "LegalCopyright" "MIT licence"

!define MUI_ICON "${SRC}\kimchi.ico"
!define MUI_UNICON "${SRC}\kimchi.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\kimchi.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open kimchi"

Var Passive

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipWhenPassive
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipWhenPassive
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  StrCpy $Passive 0
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/P" $1
  ${IfNot} ${Errors}
    StrCpy $Passive 1
    SetAutoClose true
  ${EndIf}
FunctionEnd

Function SkipWhenPassive
  ${If} $Passive == 1
    Abort
  ${EndIf}
FunctionEnd

Section "kimchi" Main
  SetOutPath "$INSTDIR"
  ; kimchi may still be closing (the updater quits it just before running this).
  Sleep 500
  SetOverwrite on
  File "${SRC}\kimchi.exe"
  File "${SRC}\kimchi-cli.exe"
  File /nonfatal "${SRC}\kimchi-mcp.exe"
  File "${SRC}\kimchi-ffmpeg.exe"
  File "${SRC}\kimchi-ffprobe.exe"
  File "${SRC}\kimchi.ico"
  File "${SRC}\FFMPEG-LICENSE.txt"
  File "${SRC}\LICENSE.txt"
  ; Left by the Tauri builds.
  Delete "$INSTDIR\resources\FFMPEG-LICENSE.txt"
  RMDir "$INSTDIR\resources"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\kimchi.exe" "" "$INSTDIR\kimchi.ico" 0

  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\kimchi.ico$\""
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "lsuite"
  WriteRegStr HKCU "${UNINSTKEY}" "URLInfoAbout" "https://lsuite.xyz/kimchi"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" $0

  ; /R (from the updater): start kimchi again.
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/R" $1
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\kimchi.exe"'
  ${EndIf}
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\kimchi.exe"
  Delete "$INSTDIR\kimchi-cli.exe"
  Delete "$INSTDIR\kimchi-mcp.exe"
  Delete "$INSTDIR\kimchi-ffmpeg.exe"
  Delete "$INSTDIR\kimchi-ffprobe.exe"
  Delete "$INSTDIR\kimchi.ico"
  Delete "$INSTDIR\FFMPEG-LICENSE.txt"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  DeleteRegKey HKCU "${UNINSTKEY}"
  ; Projects and settings (%APPDATA%\kimchi) stay.
SectionEnd
