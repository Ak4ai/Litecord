@echo off
set LITECORD_INSTANCE_ID=1
cd /d "%~dp0.litecord_instance1"
start "" "%~dp0target\debug\litecord.exe"
