Unicode True
Name "lanclip"
Caption "lanclip Setup"
OutFile "${OUTPUT_EXE}"
InstallDir "$PROGRAMFILES64\lanclip"
InstallDirRegKey HKLM "Software\lanclip" "InstallDir"
RequestExecutionLevel admin
Icon "${ICON_PATH}"
UninstallIcon "${ICON_PATH}"

!ifndef APP_VERSION
!define APP_VERSION "0.1.0"
!endif
!ifndef APP_VERSION_QUAD
!define APP_VERSION_QUAD "0.1.0.0"
!endif

SetCompressor /SOLID lzma
VIProductVersion "${APP_VERSION_QUAD}"
VIAddVersionKey /LANG=1033 "ProductName" "lanclip"
VIAddVersionKey /LANG=1033 "CompanyName" "极数本源"
VIAddVersionKey /LANG=1033 "FileDescription" "LAN clipboard and file transfer tool"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright (c) 2026 极数本源"

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetOutPath "$INSTDIR"
  File /r "${STAGE_DIR}\*.*"

  WriteRegStr HKLM "Software\lanclip" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "DisplayName" "lanclip"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "Publisher" "极数本源"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "URLInfoAbout" "https://apizero.cn/"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "DisplayIcon" "$INSTDIR\lanclip.ico"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\lanclip"
  CreateShortCut "$SMPROGRAMS\lanclip\lanclip.lnk" "$INSTDIR\lanclip.exe" "" "$INSTDIR\lanclip.ico"
  CreateShortCut "$SMPROGRAMS\lanclip\Uninstall lanclip.lnk" "$INSTDIR\Uninstall.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\lanclip\lanclip.lnk"
  Delete "$SMPROGRAMS\lanclip\Uninstall lanclip.lnk"
  RMDir "$SMPROGRAMS\lanclip"
  Delete "$INSTDIR\lanclip.exe"
  Delete "$INSTDIR\lanclip-control.exe"
  Delete "$INSTDIR\lanclip.ico"
  Delete "$INSTDIR\lanclip.svg"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\lanclip"
  DeleteRegKey HKLM "Software\lanclip"
SectionEnd
