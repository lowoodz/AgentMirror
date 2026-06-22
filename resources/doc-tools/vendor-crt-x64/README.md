# x64 MSVC runtime DLLs (vendor)

These three DLLs are copied next to bundled `pdftotext.exe` in the Windows installer
so PDF indexing works without installing Microsoft VC++ Redistributable on the target PC.

| File | Purpose |
|------|---------|
| `msvcp140.dll` | C++ standard library runtime |
| `vcruntime140.dll` | C runtime |
| `vcruntime140_1.dll` | C runtime (VS 2019+) |

All must be **PE x64 (AMD64)**. ARM64 or x86 copies cause `pdftotext.exe` to fail with
`0xc000007b` (STATUS_INVALID_IMAGE_FORMAT).

## Refresh (maintainers)

On any **x64 Windows** machine (or VM), from an extracted official redist:

```powershell
vc_redist.x64.exe /extract:C:\temp\vcrt-out /quiet /norestart
# copy x64 msvcp140.dll, vcruntime140.dll, vcruntime140_1.dll into this directory
```

Or run `./scripts/vendor/stage-vcrt-dlls.sh` with UTM Windows VM SSH available; it caches
here after a successful extract.

Microsoft permits redistributing these DLLs alongside your application (see VC++ Redist license).
