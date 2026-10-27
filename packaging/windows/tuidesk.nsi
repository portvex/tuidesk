; NSIS installer for tuidesk on Windows: a per-user install (no administrator rights) into
; %LOCALAPPDATA%\Programs\tuidesk, added to the user's PATH and to "Installed apps".
; Built by `cargo xtask dist windows`, which defines VERSION, ARCH (x86_64 or arm64), EXE (the
; built tuidesk.exe), ROOT (the repository) and OUTFILE.

Unicode true
!include "MUI2.nsh"
!include "StrFunc.nsh"
${StrStr}
${UnStrRep}

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\tuidesk"
; PATH is edited only when it fits NSIS strings with room to spare; longer ones would be cut.
!define /math PATH_LIMIT ${NSIS_MAX_STRLEN} - 300

Name "tuidesk ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\tuidesk"
InstallDirRegKey HKCU "Software\tuidesk" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "tuidesk"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "tuidesk ${VERSION} (${ARCH}) installer"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 portvex, MIT License"

!define MUI_FINISHPAGE_TEXT "tuidesk is installed. Open a new terminal (Windows Terminal is recommended) and run: tuidesk"
!insertmacro MUI_PAGE_LICENSE "${ROOT}/LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "tuidesk"
  SetOutPath "$INSTDIR"
  File "/oname=tuidesk.exe" "${EXE}"
  File "/oname=LICENSE.txt" "${ROOT}/LICENSE"
  File "${ROOT}/README.md"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "Software\tuidesk" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "tuidesk"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "portvex"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/portvex/tuidesk"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\tuidesk.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  ; tuidesk is started from a terminal, so it goes on the user's PATH.
  ReadRegStr $0 HKCU "Environment" "Path"
  StrLen $1 $0
  IntCmp $1 ${PATH_LIMIT} path_too_long path_ok path_too_long
  path_ok:
    ; Entries are compared whole, between ";" separators.
    ${StrStr} $2 ";$0;" ";$INSTDIR;"
    StrCmp $2 "" 0 path_done
    StrCmp $0 "" 0 +3
      WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR"
      Goto path_changed
    WriteRegExpandStr HKCU "Environment" "Path" "$0;$INSTDIR"
  path_changed:
    SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
    Goto path_done
  path_too_long:
    DetailPrint "Your PATH is too long to change safely; add $INSTDIR to it yourself."
  path_done:
SectionEnd

Section "Uninstall"
  ; Remove the install directory from PATH as a whole entry: wrap PATH in ";", replace
  ; ";dir;" with ";", then drop the added separators again.
  ReadRegStr $0 HKCU "Environment" "Path"
  StrLen $1 $0
  IntCmp $1 ${PATH_LIMIT} path_kept path_edit path_kept
  path_edit:
    ${UnStrRep} $0 ";$0;" ";$INSTDIR;" ";"
    StrCpy $0 $0 "" 1
    StrCpy $0 $0 -1
    WriteRegExpandStr HKCU "Environment" "Path" "$0"
    SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  path_kept:

  Delete "$INSTDIR\tuidesk.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\tuidesk"
SectionEnd
