@echo off
setlocal
title MW2 in Elden Ring - co-op setup
cd /d "%~dp0"
echo ==================================================================
echo   MW2 in Elden Ring - co-op friend setup
echo ==================================================================
echo.
echo   Builds the mod from YOUR Elden Ring and MW2 and checks that every
echo   file matches your host's. It never starts a game and never writes
echo   into the game folders. Your Elden Ring saves are backed up first.
echo.
echo   Windows boxes may ask you something (choose a file, Yes / No).
echo   Takes about 10-25 minutes. Keep this window open.
echo.
if not exist "%~dp0setup.ps1" goto notextracted
if not exist "%~dp0manifest\host_pins.json" goto notextracted
powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -File "%~dp0setup.ps1" -Interactive %*
set "RC=%ERRORLEVEL%"
echo.
if "%RC%"=="0" goto passed
echo ==================================================================
echo   SETUP FAILED (code %RC%). Read the red FAIL line above.
echo   Fix it, then double-click Install.cmd again.
echo   Help: INSTALL_GUIDE.txt (troubleshooting) and setup_logs\setup.log
echo ==================================================================
goto end
:passed
echo ==================================================================
echo   SETUP PASSED. To play: start Steam, then double-click
echo   "Play co-op.cmd" here or the desktop shortcut
echo   "MW2 in Elden Ring (co-op)". Never use the Seamless launcher.
echo ==================================================================
goto end
:notextracted
echo   This file must run from the unzipped package folder.
echo   Right-click the zip ^> Extract All..., then double-click Install.cmd
echo   in the extracted folder.
set "RC=1"
:end
echo.
pause
exit /b %RC%
