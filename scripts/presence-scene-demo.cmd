@echo off
REM Phase 0 scene demo: overlay rain in corner, then replace crossfade.
REM Requires presence-runtime built (scripts\presence-dev.cmd build -p presence-runtime).
setlocal
set "REPO=%~dp0.."
set "PROTO=%REPO%\presence-prototype"
set "BIN=%PROTO%\target\debug\presence-runtime.exe"
if not exist "%BIN%" (
  echo Build first: scripts\presence-dev.cmd build -p presence-runtime
  exit /b 1
)
set "PRESENCE_STDIN_IPC=1"
set "PRESENCE_STDOUT_IPC=1"
set "OUT=%REPO%\docs\screenshots"
if not exist "%OUT%" mkdir "%OUT%"

start "presence-runtime" /WAIT cmd /c "(
  echo {\"version\":2,\"payload\":{\"kind\":\"present_scene\",\"id\":\"precipitation\",\"params\":{\"density\":0.85,\"wind\":0.12},\"disposition\":\"overlay\",\"placement\":{\"anchor\":\"bottom_right\",\"offset\":[0,0],\"scale\":0.42},\"ttl_ms\":120000}}
  timeout /t 3 /nobreak >nul
  echo {\"version\":2,\"payload\":{\"kind\":\"dismiss_scene\",\"id\":\"precipitation\"}}
  timeout /t 2 /nobreak >nul
  echo {\"version\":2,\"payload\":{\"kind\":\"present_scene\",\"id\":\"precipitation\",\"params\":{\"density\":0.9,\"wind\":0},\"disposition\":\"replace\",\"placement\":{\"anchor\":\"center\",\"offset\":[0,0],\"scale\":0.65},\"ttl_ms\":120000}}
  timeout /t 3 /nobreak >nul
  echo {\"version\":2,\"payload\":{\"kind\":\"dismiss_scene\",\"id\":\"precipitation\"}}
  timeout /t 2 /nobreak >nul
) | \"%BIN%\""

cd /d "%PROTO%"
powershell -NoProfile -ExecutionPolicy Bypass -File capture.ps1 -Out "%OUT%\p0-overlay-rain-corner.png"
powershell -NoProfile -ExecutionPolicy Bypass -File capture.ps1 -Out "%OUT%\p0-replace-rain.png"
powershell -NoProfile -ExecutionPolicy Bypass -File capture.ps1 -Out "%OUT%\p0-idle-after-dismiss.png"
echo Screenshots in %OUT%
