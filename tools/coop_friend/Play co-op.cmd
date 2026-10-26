@echo off
setlocal
title MW2 in Elden Ring - co-op
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0launch_coop.ps1" %*
set "RC=%ERRORLEVEL%"
if "%RC%"=="0" goto end
echo.
echo   Could not start the game (code %RC%). Read the red line above.
echo   Help: INSTALL_GUIDE.txt (troubleshooting).
pause
:end
exit /b %RC%
