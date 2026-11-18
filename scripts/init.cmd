@echo off
rem The Windows entry point for scripts\init. PowerShell resolves the project
rem command `./scripts/init` here, and so does cmd for `scripts\init`. Going
rem through a .cmd file runs the bootstrap whatever the execution policy is,
rem which on a fresh machine still refuses unsigned .ps1 scripts.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0init-windows.ps1" %*
exit /b %ERRORLEVEL%
