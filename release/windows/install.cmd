@echo off
setlocal
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
if errorlevel 1 (
  echo DISP 0.1 installation failed.
  exit /b 1
)
exit /b 0

