@echo off
REM Load MSVC env and run Tauri (avoids PowerShell execution-policy blocks).
setlocal
set "REPO=%~dp0.."
set "EDGE=%REPO%\desktop-edge"

set "VSDEV="
if exist "%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat" (
  set "VSDEV=%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"
) else if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat" (
  set "VSDEV=%ProgramFiles%\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat"
) else if exist "%ProgramFiles%\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat" (
  set "VSDEV=%ProgramFiles%\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat"
)

if not defined VSDEV (
  echo Could not find VsDevCmd.bat. Install VS Build Tools with C++ workload.
  exit /b 1
)

if not exist "%EDGE%\package.json" (
  echo desktop-edge\package.json not found
  exit /b 1
)

if "%~1"=="" (
  set "NPM_ARGS=run tauri dev"
) else (
  set "NPM_ARGS=%*"
)

echo Using VS env: %VSDEV%
echo npm.cmd %NPM_ARGS%  ^(cwd=%EDGE%^)
call "%VSDEV%" -arch=amd64 -host_arch=amd64
if errorlevel 1 exit /b 1
cd /d "%EDGE%"
npm.cmd %NPM_ARGS%
exit /b %ERRORLEVEL%
