; EncodeCraft per-user installer (no admin). Built with NSIS 3.
; Pass /DPRODUCT_VERSION=0.3.0 /DOUTFILE=... /DPAYLOAD=... /DICO=...

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

!ifndef PRODUCT_VERSION
  !define PRODUCT_VERSION "0.3.0"
!endif
!ifndef PRODUCT_VERSION_NSIS
  !define PRODUCT_VERSION_NSIS "${PRODUCT_VERSION}.0"
!endif
!ifndef PAYLOAD
  !error "PAYLOAD (staged files directory) must be defined"
!endif
!ifndef OUTFILE
  !define OUTFILE "EncodeCraft-Setup-x64.exe"
!endif
!ifndef ICO
  !define ICO "encodecraft.ico"
!endif

!define PRODUCT_NAME "EncodeCraft"
!define PRODUCT_PUBLISHER "EncodeCraft"
!define PRODUCT_WEB "https://github.com/Devmaroli/encodecraft"
!define REG_UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\EncodeCraft"
!define REG_APP "Software\Craft\EncodeCraft"

Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
OutFile "${OUTFILE}"
Unicode True
SetCompressor /SOLID lzma
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Craft\EncodeCraft"
InstallDirRegKey HKCU "${REG_APP}" "InstallDir"
ShowInstDetails show
ShowUnInstDetails show
BrandingText "${PRODUCT_NAME} ${PRODUCT_VERSION}"

VIProductVersion "${PRODUCT_VERSION_NSIS}"
VIAddVersionKey /LANG=1033 "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey /LANG=1033 "FileDescription" "${PRODUCT_NAME} Setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "EncodeCraft contributors"
VIAddVersionKey /LANG=1033 "CompanyName" "${PRODUCT_PUBLISHER}"

!define MUI_ICON "${ICO}"
!define MUI_UNICON "${ICO}"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Install ${PRODUCT_NAME}"
!define MUI_WELCOMEPAGE_TEXT "This setup copies ${PRODUCT_NAME} into your user folder. You do not need administrator rights.$\r$\n$\r$\nFFmpeg is included. Shortcuts go on the Start menu and the desktop."
!define MUI_FINISHPAGE_RUN "$INSTDIR\encodecraft.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open EncodeCraft"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "EncodeCraft needs 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
FunctionEnd

Section "EncodeCraft" SecApp
  SetOutPath "$INSTDIR"
  ; Installed copy is not portable: settings stay in %APPDATA%.
  Delete "$INSTDIR\portable.txt"
  File /r "${PAYLOAD}/*"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\EncodeCraft"
  CreateShortCut "$SMPROGRAMS\EncodeCraft\EncodeCraft.lnk" "$INSTDIR\encodecraft.exe" "" "$INSTDIR\encodecraft.exe" 0
  CreateShortCut "$SMPROGRAMS\EncodeCraft\Uninstall EncodeCraft.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortCut "$DESKTOP\EncodeCraft.lnk" "$INSTDIR\encodecraft.exe" "" "$INSTDIR\encodecraft.exe" 0

  WriteRegStr HKCU "${REG_APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${REG_UNINST}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "${REG_UNINST}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "${REG_UNINST}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKCU "${REG_UNINST}" "URLInfoAbout" "${PRODUCT_WEB}"
  WriteRegStr HKCU "${REG_UNINST}" "DisplayIcon" "$INSTDIR\encodecraft.exe"
  WriteRegStr HKCU "${REG_UNINST}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${REG_UNINST}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegStr HKCU "${REG_UNINST}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "${REG_UNINST}" "NoModify" 1
  WriteRegDWORD HKCU "${REG_UNINST}" "NoRepair" 1
  WriteRegDWORD HKCU "${REG_UNINST}" "EstimatedSize" 120000
SectionEnd

Section "Uninstall"
  SetRegView 64
  Delete "$DESKTOP\EncodeCraft.lnk"
  Delete "$SMPROGRAMS\EncodeCraft\EncodeCraft.lnk"
  Delete "$SMPROGRAMS\EncodeCraft\Uninstall EncodeCraft.lnk"
  RMDir "$SMPROGRAMS\EncodeCraft"

  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR"

  DeleteRegKey HKCU "${REG_UNINST}"
  DeleteRegKey HKCU "${REG_APP}"
SectionEnd
