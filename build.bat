@echo off
cd /d "%~dp0"
rmdir /s /q build dist 2>nul
C:\Python315\python.exe -m PyInstaller --noconfirm --onefile --windowed ^
  --name MaklyxaTube ^
  --add-data "%~dp0inject.js;." ^
  --collect-submodules yt_dlp ^
  --collect-all webview ^
  --collect-all imageio_ffmpeg ^
  --exclude-module tkinter ^
  --exclude-module _tkinter ^
  --exclude-module unittest ^
  --exclude-module pydoc_data ^
  app.py
if errorlevel 1 exit /b 1
echo.
echo OK: dist\MaklyxaTube.exe
