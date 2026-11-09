@echo off
@rem Hvigor launcher for the Lite Code HarmonyOS shell.
@rem
@rem DevEco Studio ships the hvigor engine under <DevEco>\tools\hvigor.
@rem This launcher resolves that installation from DEVECO_HOME /
@rem DEVECO_SDK_HOME and falls back to a project-local
@rem hvigor\hvigor-wrapper.js if one exists.

setlocal
set "APP_HOME=%~dp0"
set "HVIGOR_JS="

if defined HVIGOR_WRAPPER_JS if exist "%HVIGOR_WRAPPER_JS%" set "HVIGOR_JS=%HVIGOR_WRAPPER_JS%"
if not defined HVIGOR_JS if exist "%APP_HOME%hvigor\hvigor-wrapper.js" set "HVIGOR_JS=%APP_HOME%hvigor\hvigor-wrapper.js"

if not defined HVIGOR_JS if defined DEVECO_HOME if exist "%DEVECO_HOME%\tools\hvigor\bin\hvigorw.js" set "HVIGOR_JS=%DEVECO_HOME%\tools\hvigor\bin\hvigorw.js"
if not defined HVIGOR_JS if defined DEVECO_SDK_HOME if exist "%DEVECO_SDK_HOME%\..\tools\hvigor\bin\hvigorw.js" set "HVIGOR_JS=%DEVECO_SDK_HOME%\..\tools\hvigor\bin\hvigorw.js"

if not defined HVIGOR_JS (
  echo ERROR: hvigorw.js was not found. 1>&2
  echo Set DEVECO_SDK_HOME ^(for example D:\ide\Huawei\DevEco Studio\sdk^) 1>&2
  echo or DEVECO_HOME to the DevEco Studio installation directory. 1>&2
  exit /b 1
)

set "NODE_BIN=node.exe"
if defined NODE_HOME if exist "%NODE_HOME%\bin\node.exe" set "NODE_BIN=%NODE_HOME%\bin\node.exe"
if defined NODE_HOME if exist "%NODE_HOME%\node.exe" set "NODE_BIN=%NODE_HOME%\node.exe"

pushd "%APP_HOME%" >NUL
"%NODE_BIN%" "%HVIGOR_JS%" %*
set "HVIGOR_EXIT=%ERRORLEVEL%"
popd >NUL
exit /b %HVIGOR_EXIT%
