@echo off
set LITECORD_INSTANCE_ID=2
cd /d "%~dp0.litecord_instance2"
start "" "%~dp0target\debug\litecord.exe"
