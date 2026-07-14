@echo off
setlocal EnableExtensions EnableDelayedExpansion

if not defined CARGO set "CARGO=cargo"

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

%CARGO% build --release --locked
exit /b %errorlevel%
