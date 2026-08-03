@echo off
setlocal
set "REPO=%~dp0.."
set "VSDEV=%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"
if not exist "%VSDEV%" exit /b 1
call "%VSDEV%" -arch=amd64 -host_arch=amd64
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
cd /d "%REPO%"
cargo %*
