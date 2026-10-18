@echo off
setlocal EnableExtensions EnableDelayedExpansion

if not defined CARGO set "CARGO=cargo"

rem Cargo fingerprints native build-script environment variables such as
rem VCINSTALLDIR. Every repository Cargo command must therefore enter through
rem the same MSVC environment, or alternating lint/test/build commands evict
rem one another's cached native artifacts.
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

rem Git Bash can prepend usr\bin after MSVC was initialized. VSCMD_VER then
rem remains set even though link.exe resolves to Git's unrelated file utility.
rem Restore the tool directory on every invocation, including that fast path.
if defined VCToolsInstallDir (
    if not exist "!VCToolsInstallDir!bin\Hostx64\x64\link.exe" (
        echo Error: The initialized MSVC environment has no x64 linker. 1>&2
        exit /b 1
    )
    set "PATH=!VCToolsInstallDir!bin\Hostx64\x64;!PATH!"
)

rem Set priority before Cargo starts, so compiler, test and helper processes
rem inherit it too. /b keeps the caller's console and /wait preserves failures.
start "" /b /belownormal /wait %CARGO% %*
exit /b !errorlevel!
