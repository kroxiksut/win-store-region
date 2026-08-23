Portable executables, no installer and no administrator rights. Pick the one
that matches the machine:

| File | For |
|---|---|
| `WinStoreRegion-{version}-x64.exe` | 64-bit Intel or AMD — the usual case |
| `WinStoreRegion-{version}-arm64.exe` | ARM64 devices |
| `WinStoreRegion-{version}-x86.exe` | 32-bit Windows |

**Only the x64 build has ever been run.** All three are built by the same
workflow from this tag, so all three compile; the other two have never been
started on a real device. They are here because a machine that can run them is
the only way that changes — not because this release says they work. If you try
one, an issue saying what happened is the useful outcome either way.

**The binaries are not signed.** Windows shows "Windows protected your PC" on
first run and puts the button behind **More info → Run anyway**. A downloaded
file also carries a mark that keeps SmartScreen involved after extraction;
`Unblock-File` removes that mark. Only a code-signing certificate removes the
warning itself, and this release does not have one.

What can be checked is identity. `SHA256SUMS.txt` lists the digest of every file,
and the workflow run that produced them is public:

```powershell
Get-FileHash .\WinStoreRegion-{version}-x64.exe -Algorithm SHA256
```

What changed is in [CHANGELOG.md](CHANGELOG.md). What the program does, and what
it has and has not established, is in [README.md](README.md) — also in
[Russian](README.ru.md) and [Chinese](README.zh-CN.md).
