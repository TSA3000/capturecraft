; CaptureCraft NSIS installer.
; Built by .github/workflows/release.yml; VERSION, ARCH and SOURCE come in
; on the command line (-D), so the same script serves x64 and ARM64.

Unicode true
ManifestDPIAware true

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef ARCH
  !define ARCH "x64"
!endif
!ifndef SOURCE
  !define SOURCE "."
!endif
; Absolute, because NSIS resolves relative paths against this script's folder.
!ifndef OUTDIR
  !define OUTDIR "."
!endif

!define APP "CaptureCraft"
!define PUBLISHER "TSA3000"
!define REGKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"

Name "${APP} ${VERSION}"
OutFile "${OUTDIR}\capturecraft-${VERSION}-${ARCH}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "Software\${APP}" "InstallDir"
; Per-user install: no admin prompt, like Snagit's user-mode install.
RequestExecutionLevel user
SetCompressor /SOLID lzma

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Function .onInit
  ; An upgrade must not fight a running copy for its own files.
  nsExec::Exec 'taskkill /IM capturecraft-tray.exe /F'
  nsExec::Exec 'taskkill /IM capturecraft-app.exe /F'
  nsExec::Exec 'taskkill /IM capturecraft-editor.exe /F'
FunctionEnd

Section "Install"
  SetOutPath "$INSTDIR"
  File "${SOURCE}\capturecraft.exe"
  File "${SOURCE}\capturecraft-app.exe"
  File "${SOURCE}\capturecraft-tray.exe"
  File "${SOURCE}\capturecraft-editor.exe"
  File "${SOURCE}\README.md"
  File "${SOURCE}\LICENSE"

  CreateDirectory "$SMPROGRAMS\${APP}"
  CreateShortCut "$SMPROGRAMS\${APP}\${APP}.lnk" "$INSTDIR\capturecraft-app.exe"
  CreateShortCut "$SMPROGRAMS\${APP}\${APP} Editor.lnk" "$INSTDIR\capturecraft-editor.exe"
  CreateShortCut "$DESKTOP\${APP}.lnk" "$INSTDIR\capturecraft-app.exe"

  WriteRegStr HKCU "Software\${APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${REGKEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${REGKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${REGKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${REGKEY}" "DisplayIcon" "$INSTDIR\capturecraft-app.exe"
  WriteRegStr HKCU "${REGKEY}" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${REGKEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${REGKEY}" "NoRepair" 1
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /IM capturecraft-tray.exe /F'
  nsExec::Exec 'taskkill /IM capturecraft-app.exe /F'
  nsExec::Exec 'taskkill /IM capturecraft-editor.exe /F'

  Delete "$INSTDIR\capturecraft.exe"
  Delete "$INSTDIR\capturecraft-app.exe"
  Delete "$INSTDIR\capturecraft-tray.exe"
  Delete "$INSTDIR\capturecraft-editor.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP}\${APP}.lnk"
  Delete "$SMPROGRAMS\${APP}\${APP} Editor.lnk"
  RMDir "$SMPROGRAMS\${APP}"
  Delete "$DESKTOP\${APP}.lnk"

  ; Autostart entry the app may have written (Advanced > Start with Windows).
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "CaptureCraft"
  DeleteRegKey HKCU "${REGKEY}"
  DeleteRegKey HKCU "Software\${APP}"
SectionEnd
