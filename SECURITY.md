# Security policy

SYNCFASTERGB is **not independently audited** and is provided as-is. It is a small local
tray app: no networking, no telemetry, no elevation. It reads screen pixels in memory
and writes colors to one USB HID device.

## Reporting a vulnerability

Please report privately through GitHub: **Security → Report a vulnerability** on this
repository. Do not open a public issue for security problems.

Useful details: version, Windows version, steps to reproduce, and impact.
You will get an acknowledgement as soon as the maintainer can respond; this is a
hobby project, so there is no guaranteed turnaround.

## Threat model (summary)

| Area | Position |
|---|---|
| Network | None. CI bans HTTP/TLS/socket crates (`cargo-deny`). |
| Privilege | Never requires admin; never spawns elevated processes or shells. |
| Screen content | Downscaled in memory; never written to disk or logged. |
| Config file | User-writable; size-capped, parsed strictly, every value range-clamped. |
| USB device | Only VID `0x1A86` / PID `0xFE07`; HID replies are bounds-checked. |
| Supply chain | `Cargo.lock` committed; builds use `--locked`; Actions pinned by SHA; Dependabot enabled. |
| Releases | SHA-256 checksums and GitHub build-provenance attestations. |

## Verifying a release

```powershell
(Get-FileHash .\syncfastergb.exe -Algorithm SHA256).Hash   # compare with SHA256SUMS.txt
gh attestation verify .\syncfastergb.exe --repo IssamBouchahine/SYNCFASTERGB
```

## Known limitations

- Releases are not Authenticode-signed, so Windows SmartScreen may warn on first run.
- A rogue USB device that imitates the strip's VID/PID can receive color data and send
  malformed replies; replies are bounds-checked, but treat unknown USB devices with care.
