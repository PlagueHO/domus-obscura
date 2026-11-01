Unicode true
!include "MUI2.nsh"
!include "x64.nsh"
!include "checked-commands.nsh"

!define HELPER '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -File'

!ifndef PAYLOAD
!error "PAYLOAD must identify the verified package directory"
!endif
!ifndef OUTPUT
!error "OUTPUT must identify the installer output"
!endif

Name "Domus Obscura"
OutFile "${OUTPUT}"
InstallDir "$PROGRAMFILES64\Domus Obscura"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
    ${IfNot} ${RunningX64}
        MessageBox MB_OK|MB_ICONSTOP "This package requires Windows x64."
        Abort
    ${EndIf}
    SetRegView 64
    StrCpy $INSTDIR "$PROGRAMFILES64\Domus Obscura"
FunctionEnd

Function CheckPrevious
    StrCmp $INSTDIR "$PROGRAMFILES64\Domus Obscura" location_ok
    MessageBox MB_OK|MB_ICONSTOP "Use the fixed Program Files installation directory. Keep writable application state elsewhere."
    Abort
location_ok:
    InitPluginsDir
    SetOutPath "$PLUGINSDIR"
    File /oname=services.ps1 "${PAYLOAD}\services.ps1"
    File /oname=incoming-manifest.json "${PAYLOAD}\package-manifest.json"
    !insertmacro ExecChecked '${HELPER} "$PLUGINSDIR\services.ps1" -Action CheckInstallPath -InstallationDirectory "$INSTDIR"' stopped path_failed
path_failed:
    MessageBox MB_OK|MB_ICONSTOP "The installation path or stopped-process check failed. Inspect the path and stop all installation processes before retrying. No files were replaced."
    Abort
stopped:
    IfFileExists "$INSTDIR\*.*" 0 done
    IfFileExists "$INSTDIR\domus.exe" 0 unknown
    IfFileExists "$INSTDIR\services.ps1" 0 unknown
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "Before updating, make a consistent SQLite backup with both services stopped. Have you completed and checked this backup?" IDYES prepare
    Abort
prepare:
    !insertmacro ExecChecked '${HELPER} "$PLUGINSDIR\services.ps1" -Action PrepareUpdate -InstallationDirectory "$INSTDIR" -IncomingManifest "$PLUGINSDIR\incoming-manifest.json"' done update_failed
update_failed:
    MessageBox MB_OK|MB_ICONSTOP "Update preparation failed. Changed files were preserved. Inspect the existing installation before retrying."
    Abort
unknown:
    MessageBox MB_OK|MB_ICONSTOP "The fixed installation directory contains unknown files. Inspect them before retrying. Existing files were preserved."
    Abort
done:
FunctionEnd

Section "Application (foreground use)" Application
    SectionIn RO
    Call CheckPrevious
    SetOutPath "$INSTDIR"
    File "${PAYLOAD}\domus.exe"
    File "${PAYLOAD}\services.ps1"
    File "${PAYLOAD}\windows-services.md"
    File "${PAYLOAD}\package-manifest.json"
    File "${PAYLOAD}\uninstall-files.nsh"
    SetOutPath "$INSTDIR\browser"
    File /r "${PAYLOAD}\browser\*"
    SetOutPath "$INSTDIR\migrations"
    File /r "${PAYLOAD}\migrations\*"
    SetOutPath "$INSTDIR\notices"
    File /r "${PAYLOAD}\notices\*"
    SetOutPath "$INSTDIR\build-provenance"
    File /r "${PAYLOAD}\build-provenance\*"
    SetOutPath "$INSTDIR"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    !insertmacro ExecChecked '${HELPER} "$INSTDIR\services.ps1" -Action RecordInstall' recorded record_failed
record_failed:
    MessageBox MB_OK|MB_ICONSTOP "The installation record could not be verified. Program files remain for inspection. No application data was removed."
    Abort
recorded:
    ClearErrors
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\DomusObscura" "DisplayName" "Domus Obscura"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\DomusObscura" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\DomusObscura" "InstallLocation" "$INSTDIR"
    SetShellVarContext all
    CreateDirectory "$SMPROGRAMS\Domus Obscura"
    CreateShortCut "$SMPROGRAMS\Domus Obscura\Domus Obscura.lnk" "$INSTDIR\domus.exe" "serve" "$INSTDIR\domus.exe"
    CreateShortCut "$SMPROGRAMS\Domus Obscura\Operator guide.lnk" "$INSTDIR\windows-services.md"
    IfErrors application_failed done
application_failed:
    MessageBox MB_OK|MB_ICONSTOP "The installation record or shortcuts could not be created. Setup is incomplete. Program files remain for inspection."
    SetErrorLevel 1
    Abort
done:
SectionEnd

Section /o "Configure optional Windows services" Services
    MessageBox MB_OK|MB_ICONINFORMATION "Service setup requires an existing dedicated non-administrator local account and an absolute backend configuration. The next prompt collects credentials securely. Setup does not change path permissions, start services, or approve media work."
    !insertmacro ExecChecked '${HELPER} "$INSTDIR\services.ps1" -Action Install' done setup_failed
setup_failed:
    MessageBox MB_OK|MB_ICONSTOP "Service setup failed or was cancelled. The application and any partial service registration are preserved. Inspect the operator guide before retrying."
    SetErrorLevel 1
done:
SectionEnd

Section "Uninstall"
    SetRegView 64
    !insertmacro ExecChecked '${HELPER} "$INSTDIR\services.ps1" -Action ValidateUninstall' stopped validation_failed
validation_failed:
    MessageBox MB_OK|MB_ICONSTOP "Uninstall validation failed. Stop all installation processes and inspect changed files or reparse paths before retrying. No files or services were removed."
    Abort
stopped:
    !insertmacro ExecChecked '${HELPER} "$INSTDIR\services.ps1" -Action Remove' removed services_failed
services_failed:
    MessageBox MB_OK|MB_ICONSTOP "Service removal failed. Program files and all data were preserved."
    Abort
removed:
    ; Generated at package time from exact installer-owned relative paths.
    !include "${PAYLOAD}\uninstall-files.nsh"
    !insertmacro DeleteChecked "$INSTDIR\Uninstall.exe" removal_failed
    RMDir "$INSTDIR"
    SetShellVarContext all
    !insertmacro DeleteChecked "$SMPROGRAMS\Domus Obscura\Domus Obscura.lnk" removal_failed
    !insertmacro DeleteChecked "$SMPROGRAMS\Domus Obscura\Operator guide.lnk" removal_failed
    RMDir "$SMPROGRAMS\Domus Obscura"
    ClearErrors
    DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\DomusObscura"
    IfErrors registration_failed
    Goto done
removal_failed:
    MessageBox MB_OK|MB_ICONSTOP "A program file or shortcut could not be removed. Removal is incomplete. Inspect the remaining installation files before retrying. Application data was preserved."
    SetErrorLevel 1
    Abort
registration_failed:
    MessageBox MB_OK|MB_ICONSTOP "The uninstall record could not be removed. Removal is incomplete. Inspect Windows application registration before retrying."
    SetErrorLevel 1
    Abort
done:
SectionEnd
