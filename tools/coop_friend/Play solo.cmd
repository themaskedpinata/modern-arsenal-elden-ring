@echo off
setlocal
title MW2 in Elden Ring - solo
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0launch_solo.ps1" %*
set "RC=%ERRORLEVEL%"
if "%RC%"=="0" goto end
echo.
echo   Could not start the game (code %RC%). Read the red line above.
echo   Help: INSTALL_GUIDE.txt (troubleshooting).
pause
:end
exit /b %RC%
