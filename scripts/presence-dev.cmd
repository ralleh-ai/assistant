@echo off
REM Load MSVC env and run cargo inside presence-prototype/ (avoids
REM PowerShell execution-policy blocks and this machine's default-shell
REM linker-lib issue — see presence-prototype/README.md).
setlocal
set "REPO=%~dp0.."
set "PROTO=%REPO%\presence-prototype"

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

if not exist "%PROTO%\Cargo.toml" (
  echo presence-prototype\Cargo.toml not found
  exit /b 1
)

if "%~1"=="" (
  set "CARGO_ARGS=run"
) else (
  set "CARGO_ARGS=%*"
)

echo Using VS env: %VSDEV%
echo cargo %CARGO_ARGS%  ^(cwd=%PROTO%^)
call "%VSDEV%" -arch=amd64 -host_arch=amd64
if errorlevel 1 exit /b 1
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
cd /d "%PROTO%"
cargo %CARGO_ARGS%
exit /b %ERRORLEVEL%
