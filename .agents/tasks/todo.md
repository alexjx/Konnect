# Rebuild, restart, and install skills — 2026-08-21

- [x] Identify the configured Konnect MCP executable and running process
- [x] Build and verify the release binary
- [x] Replace the configured executable without disturbing unrelated processes
- [x] Install all bundled skills through the rebuilt binary
- [x] Restart Konnect and verify its process, version, MCP handshake, and skills
- [x] Commit the verified source and skill updates

## Review

- Built release `konnect 0.1.3-xinj.40` and deployed it to the exact configured
  path `D:\wkspace\Konnect\konnect.exe`.
- Stopped only `konnect.exe` processes whose resolved executable path matched
  the configured target. Preserved the previous executable at
  `D:\wkspace\Konnect\konnect.exe.pre-rebuild-20260821-2.bak`.
- Release and deployed SHA-256 hashes match:
  `5B33491B40196310C055AD81EAF4AB900F0734021C863B088E4F6600F49ABE72`.
- The rebuilt binary's installer reports all three bundled skills present:
  `kicad-layout-review`, `konnect-kicad-schematic`, and
  `konnect-kicad-pcb-layout`. The separately maintained
  `kicad-package-audit` also matches its repository files.
- Verified a fresh stdio process with an MCP initialize and `tools/list`
  handshake; it reported server version `0.1.3-xinj.40` and exited normally
  when the smoke client's stdin closed.
- The pre-existing MCP connection for this task closed when its old process was
  stopped and cannot reattach in place. A new task connection or Codex app
  reload will spawn the rebuilt executable.
