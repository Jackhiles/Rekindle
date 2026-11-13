@echo off
rem Build Rekindle and put Rekindle.exe in this folder (double-click it to open the launcher).
setlocal
cd /d "%~dp0"
where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
cargo build --release || goto :failed
rem A running Rekindle.exe can't be overwritten, but it can be moved aside;
rem Rekindle deletes Rekindle.old.exe the next time it starts.
if exist Rekindle.exe (
    del /q Rekindle.old.exe 2>nul
    move /y Rekindle.exe Rekindle.old.exe >nul
)
copy /y target\release\rekindle.exe Rekindle.exe >nul || goto :failed
echo.
echo Built Rekindle.exe - double-click it to open the launcher.
exit /b 0
:failed
echo.
echo Build failed.
pause
exit /b 1
