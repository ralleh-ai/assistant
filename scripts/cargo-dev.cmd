@echo off
REM Load MSVC env and run cargo in the root workspace. Same reason as
REM scripts\presence-dev.cmd: the default PowerShell on this machine does
REM not have LIB/INCLUDE set, so `cargo build` fails to find `msvcrt.lib`.
REM Use for the headless workspace at the repo root
REM (`cargo build --workspace`, `cargo test -p presence-ipc`, etc.).
setlocal
set "REPO=%~dp0.."

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

if not exist "%REPO%\Cargo.toml" (
  echo repo-root Cargo.toml not found
  exit /b 1
)

if "%~1"=="" (
  set "CARGO_ARGS=build --workspace"
) else (
  set "CARGO_ARGS=%*"
)

echo Using VS env: %VSDEV%
echo cargo %CARGO_ARGS%  ^(cwd=%REPO%^)
call "%VSDEV%" -arch=amd64 -host_arch=amd64
if errorlevel 1 exit /b 1
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
cd /d "%REPO%"
cargo %CARGO_ARGS%
exit /b %ERRORLEVEL%
