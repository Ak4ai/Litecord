@echo off
echo Iniciando Instancia 1 (Perfil 1)...
set LITECORD_INSTANCE_ID=1
start "" /D "%~dp0.litecord_instance1" "%~dp0target\debug\litecord.exe"

timeout /t 1 /nobreak >nul

echo Iniciando Instancia 2 (Perfil 2)...
set LITECORD_INSTANCE_ID=2
start "" /D "%~dp0.litecord_instance2" "%~dp0target\debug\litecord.exe"

echo Duas instancias do Litecord iniciadas com sucesso!
