Portable `WinStoreRegion-{version}-x64.exe`. No installer, no administrator
rights.

**The binary is not signed.** Windows shows "Windows protected your PC" on first
run and puts the button behind **More info → Run anyway**. A downloaded file
also carries a mark that keeps SmartScreen involved after extraction;
`Unblock-File .\WinStoreRegion-{version}-x64.exe` removes that mark. Only a
code-signing certificate removes the warning itself, and this release does not
have one.

What can be checked is identity. `SHA256SUMS.txt` below lists the digest, and
the workflow run that produced these files is public:

```powershell
Get-FileHash .\WinStoreRegion-{version}-x64.exe -Algorithm SHA256
```

**x64 only.** ARM64 and 32-bit x86 are built on every push and can be downloaded
from the workflow run, but neither has ever been started on a real device, so
neither is offered here as something to install.

What changed is in [CHANGELOG.md](CHANGELOG.md). What the program does, and what
it has and has not established, is in [README.md](README.md) — also in
[Russian](README.ru.md) and [Chinese](README.zh-CN.md).
