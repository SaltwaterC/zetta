@echo off
setlocal EnableExtensions EnableDelayedExpansion

goto :main

:append_feature
if /i "%~1"=="0" exit /b 0
if /i "%~1"=="false" exit /b 0
if /i "%~1"=="no" exit /b 0
if /i "%~1"=="off" exit /b 0
set "FEATURES=%FEATURES%,%~2"
exit /b 0

:append_worktree_targets
if /i "%~1"=="0" exit /b 0
if /i "%~1"=="false" exit /b 0
if /i "%~1"=="no" exit /b 0
if /i "%~1"=="off" exit /b 0
set "BINARIES=!BINARIES! --bin zwt"
set "VERIFY_ARGS=!VERIFY_ARGS! -WorktreeBinaryPath !TARGET_DIR!\zwt.exe"
exit /b 0

:append_zmux_targets
if /i "%~1"=="0" exit /b 0
if /i "%~1"=="false" exit /b 0
if /i "%~1"=="no" exit /b 0
if /i "%~1"=="off" exit /b 0
set "BINARIES=!BINARIES! --bin zmux --bin zmux-pty"
set "VERIFY_ARGS=!VERIFY_ARGS! -MuxBinaryPath !TARGET_DIR!\zmux.exe -PtyBinaryPath !TARGET_DIR!\zmux-pty.exe"
exit /b 0

:append_zosh_client_targets
if /i "%~1"=="0" exit /b 0
if /i "%~1"=="false" exit /b 0
if /i "%~1"=="no" exit /b 0
if /i "%~1"=="off" exit /b 0
set "BINARIES=!BINARIES! --bin zosh"
set "VERIFY_ARGS=!VERIFY_ARGS! -ZoshBinaryPath !TARGET_DIR!\zosh.exe"
exit /b 0

:append_zosh_server_targets
if /i "%~1"=="0" exit /b 0
if /i "%~1"=="false" exit /b 0
if /i "%~1"=="no" exit /b 0
if /i "%~1"=="off" exit /b 0
set "BINARIES=!BINARIES! --bin zosh-server"
set "VERIFY_ARGS=!VERIFY_ARGS! -ZoshServerBinaryPath !TARGET_DIR!\zosh-server.exe"
exit /b 0

:main
if not defined CARGO set "CARGO=cargo"
if not defined SERIAL set "SERIAL=1"
if not defined HTTP set "HTTP=1"
if not defined TFTP set "TFTP=1"
if not defined TFTP_SERVER set "TFTP_SERVER=%TFTP%"
if not defined TFTP_CLIENT set "TFTP_CLIENT=%TFTP%"
if not defined CLIPBOARD set "CLIPBOARD=1"
if not defined NOTIFY set "NOTIFY=1"
if not defined SYNTAX_HIGHLIGHTING set "SYNTAX_HIGHLIGHTING=1"
if not defined ZMUX set "ZMUX=1"
if not defined SESSION_PERSISTENCE set "SESSION_PERSISTENCE=1"
if not defined WORKTREE set "WORKTREE=1"
if not defined ZOSH set "ZOSH=1"
if not defined ZOSH_CLIENT set "ZOSH_CLIENT=%ZOSH%"
if not defined ZOSH_SERVER set "ZOSH_SERVER=%ZOSH%"

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
call :append_feature "%ZMUX%" zmux
if /i not "%ZMUX%"=="0" if /i not "%ZMUX%"=="false" if /i not "%ZMUX%"=="no" if /i not "%ZMUX%"=="off" call :append_feature "%SESSION_PERSISTENCE%" session-persistence
call :append_feature "%WORKTREE%" worktree
call :append_feature "%ZOSH_CLIENT%" zosh-client
call :append_feature "%ZOSH_SERVER%" zosh-server

set "BINARIES=--bin zetta --bin zetta-gui"
set "VERIFY_ARGS="
call :append_zmux_targets "%ZMUX%"
call :append_zosh_client_targets "%ZOSH_CLIENT%"
call :append_zosh_server_targets "%ZOSH_SERVER%"
call :append_worktree_targets "%WORKTREE%"

call scripts\cargo-windows.cmd build %PROFILE_ARGS% --jobs %CARGO_BUILD_JOBS% --locked --no-default-features --features %FEATURES% !BINARIES!
if errorlevel 1 exit /b !errorlevel!

powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\verify-windows-binary.ps1 -ConsoleBinaryPath !TARGET_DIR!\zetta.exe -GuiBinaryPath !TARGET_DIR!\zetta-gui.exe !VERIFY_ARGS!
exit /b !errorlevel!
