@echo off
echo 这是一个测试脚本
echo 它会每秒打印一次时间
echo.
echo 按 Ctrl+C 可以停止
echo.

:loop
echo 当前时间: %time%
timeout /t 1 /nobreak >nul
goto loop
