Place the bundled Windows mpv sidecar here before packaging:

- `mpv-x86_64-pc-windows-msvc.exe`

`tauri.conf.json` registers this sidecar as `binaries/mpv`, so packaged builds and local sidecar launches resolve through that name.
