@echo off
setlocal EnableExtensions EnableDelayedExpansion

if not defined CARGO set "CARGO=cargo"
if not defined SERIAL set "SERIAL=1"
if not defined HTTP set "HTTP=1"
if not defined TFTP set "TFTP=1"
if not defined TFTP_SERVER set "TFTP_SERVER=%TFTP%"
if not defined TFTP_CLIENT set "TFTP_CLIENT=%TFTP%"
if not defined CLIPBOARD set "CLIPBOARD=1"
if not defined NOTIFY set "NOTIFY=1"
if not defined SYNTAX_HIGHLIGHTING set "SYNTAX_HIGHLIGHTING=1"

set "FEATURES=windows-gui"
set "PROFILE_ARGS="
set "TARGET_DIR=target\debug"
if /i "%~1"=="--release" (
    set "PROFILE_ARGS=--release"
    set "TARGET_DIR=target\release"
)

if not defined CARGO_BUILD_JOBS (
    for /f %%I in ('powershell.exe -NoProfile -Command "[Environment]::ProcessorCount"') do set "CARGO_BUILD_JOBS=%%I"
)

call :append_feature "%SERIAL%" serial-console
call :append_feature "%HTTP%" http-server
call :append_feature "%TFTP_SERVER%" tftp-server
call :append_feature "%TFTP_CLIENT%" tftp-client
call :append_feature "%CLIPBOARD%" clipboard
call :append_feature "%NOTIFY%" notifications
call :append_feature "%SYNTAX_HIGHLIGHTING%" syntax-highlighting

if not defined VSCMD_VER (
    set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"

    if not exist "!VSWHERE!" (
        echo Error: Visual Studio Installer's vswhere.exe was not found. 1>&2
        echo Install the Visual Studio Desktop development with C++ workload. 1>&2
        exit /b 1
    )

    for /f "usebackq delims=" %%I in (`"!VSWHERE!" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%I"

    if not defined VSINSTALL (
        echo Error: A Visual Studio installation with the C++ build tools was not found. 1>&2
        echo Install the Visual Studio Desktop development with C++ workload. 1>&2
        exit /b 1
    )

    call "!VSINSTALL!\VC\Auxiliary\Build\vcvars64.bat" >nul
    if errorlevel 1 exit /b !errorlevel!
)

%CARGO% build %PROFILE_ARGS% --jobs %CARGO_BUILD_JOBS% --locked --no-default-features --features %FEATURES% --bin zetta --bin zetta-gui
if errorlevel 1 exit /b !errorlevel!

powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\verify-windows-binary.ps1 -ConsoleBinaryPath !TARGET_DIR!\zetta.exe -GuiBinaryPath !TARGET_DIR!\zetta-gui.exe
exit /b !errorlevel!

:append_feature
if /i "%~1"=="0" exit /b 0
if /i "%~1"=="false" exit /b 0
if /i "%~1"=="no" exit /b 0
if /i "%~1"=="off" exit /b 0
set "FEATURES=%FEATURES%,%~2"
exit /b 0
