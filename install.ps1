# Retired installer. The Fugue command-line tools and MCP server are installed
# from the Fugue product release now, so this file only points there: it prints
# the install command and fails. It makes no network request and writes no
# files, so running it from the old one-line command is safe.
#
# Keep it at the repository root: users still have the old one-line command
# (https://raw.githubusercontent.com/gdamron/fugue/main/install.ps1) in scripts
# and notes.
#
# Keep the file ASCII and compatible with Windows PowerShell 5.1 and PowerShell 7.
# It ends with `throw` rather than `exit`: the old command pipes this file to
# `iex`, and `exit` there would close the user's PowerShell window before the
# message could be read. An uncaught `throw` still gives a non-zero exit code to
# `powershell -File`, `-Command` and `pwsh` alike.

$message = @'
error: this installer has moved. Install Fugue from the product release instead:

  powershell -ExecutionPolicy ByPass -c "irm https://github.com/Ilusiv-media/fyug/releases/latest/download/install.ps1 | iex"

Nothing was installed.
'@

$host.ui.WriteErrorLine($message)
throw 'install.ps1 has moved.'
