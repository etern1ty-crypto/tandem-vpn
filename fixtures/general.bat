@echo off
rem Parser-only fixture. Not a deployable engine bundle.
echo This file is test data. Use tandem inspect instead of executing it.
exit /b 1
start "Tandem parser fixture" /min "%BIN%winws.exe" --wf-tcp=443 ^
--filter-tcp=443 --dpi-desync=fake
