@echo off
setlocal
cd /d "%~dp0"
cargo build --locked --release
if errorlevel 1 exit /b 1
if not exist dist mkdir dist
if errorlevel 1 exit /b 1
copy /y target\release\maklyxa-tube.exe dist\MaklyxaTube.exe >nul
if errorlevel 1 exit /b 1
echo OK: dist\MaklyxaTube.exe
