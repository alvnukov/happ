!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "StrFunc.nsh"
!include "WinMessages.nsh"

${StrLen}
${StrStr}
${StrRep}

!ifndef VERSION
  !define VERSION "0.0.0-dev"
!endif

!ifndef INPUT_BIN
  !error "INPUT_BIN define is required"
!endif

!ifndef OUTPUT_EXE
  !define OUTPUT_EXE "dist\happ_windows_amd64_installer.exe"
!endif

!define APP_NAME "happ"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\happ"
!define APP_REG_KEY "Software\happ"
!define ENV_REG_KEY "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
!define MUI_FINISHPAGE_NOAUTOCLOSE

Name "${APP_NAME}"
OutFile "${OUTPUT_EXE}"
Unicode True
RequestExecutionLevel admin
InstallDir "$PROGRAMFILES64\happ"
InstallDirRegKey HKLM "${APP_REG_KEY}" "InstallDir"
BrandingText "${APP_NAME} ${VERSION}"

!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Core files (required)" SecCore
  SectionIn RO
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "/oname=happ.exe" "${INPUT_BIN}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  WriteRegStr HKLM "${APP_REG_KEY}" "InstallDir" "$INSTDIR"

  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "alvnukov"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Add happ to PATH (recommended)" SecPath
  SetRegView 64
  Call AddInstallDirToPath
SectionEnd

Section "Uninstall"
  SetRegView 64
  Call un.RemoveInstallDirFromPath
  Delete "$INSTDIR\happ.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "${APP_REG_KEY}"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} "Install happ CLI binary."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPath} "Add happ install directory to system PATH."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function RefreshEnvironment
  System::Call 'user32::SendMessageTimeoutW(i ${HWND_BROADCAST}, i ${WM_SETTINGCHANGE}, i 0, w "Environment", i 0x2, i 5000, *i .r0)'
FunctionEnd

Function AddInstallDirToPath
  ReadRegStr $0 HKLM "${ENV_REG_KEY}" "Path"
  ${If} $0 == ""
    StrCpy $0 "$INSTDIR"
    Goto add_path_write
  ${EndIf}

  StrCpy $1 ";$0;"
  StrCpy $2 ";$INSTDIR;"
  ${StrStr} $3 $1 $2
  ${If} $3 != ""
    Return
  ${EndIf}
  StrCpy $0 "$0;$INSTDIR"

add_path_write:
  WriteRegExpandStr HKLM "${ENV_REG_KEY}" "Path" "$0"
  Call RefreshEnvironment
FunctionEnd

Function un.RemoveInstallDirFromPath
  ReadRegStr $0 HKLM "${ENV_REG_KEY}" "Path"
  ${If} $0 == ""
    Return
  ${EndIf}

  StrCpy $1 ";$0;"

remove_entry_loop:
  ${StrRep} $2 "$1" ";$INSTDIR;" ";"
  ${If} $2 == $1
    Goto normalize_separators
  ${EndIf}
  StrCpy $1 "$2"
  Goto remove_entry_loop

normalize_separators:
  ${StrRep} $2 "$1" ";;" ";"
  ${If} $2 == $1
    Goto trim_prefix
  ${EndIf}
  StrCpy $1 "$2"
  Goto normalize_separators

trim_prefix:
  StrCpy $2 $1 1
  ${If} $2 == ";"
    StrCpy $1 $1 "" 1
  ${EndIf}

trim_suffix:
  ${StrLen} $3 $1
  ${If} $3 > 0
    IntOp $3 $3 - 1
    StrCpy $2 $1 1 $3
    ${If} $2 == ";"
      StrCpy $1 $1 $3
    ${EndIf}
  ${EndIf}

  WriteRegExpandStr HKLM "${ENV_REG_KEY}" "Path" "$1"
  Call un.RefreshEnvironment
FunctionEnd

Function un.RefreshEnvironment
  System::Call 'user32::SendMessageTimeoutW(i ${HWND_BROADCAST}, i ${WM_SETTINGCHANGE}, i 0, w "Environment", i 0x2, i 5000, *i .r0)'
FunctionEnd
