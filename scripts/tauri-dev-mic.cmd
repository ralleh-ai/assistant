@echo off
REM Tauri dev with live mic (cpal / WASAPI) enabled.
REM `--features` must be a tauri CLI flag (before any bare `--`), not a runner arg.
REM Requires station-log Voice clearance before Mic smoke works.
call "%~dp0tauri-dev.cmd" run tauri -- dev --features mic
