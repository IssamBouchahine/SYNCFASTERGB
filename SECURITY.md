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

## The strip also presents itself as a keyboard

The supported strip (USB `1A86:FE07`) is a composite device: interface 0 is the vendor-defined
LED channel this app uses, and interface 1 is a **HID keyboard**. Its firmware uses that keyboard
to press Win+R and type a command (it opens the vendor's software download page) every time
the strip is plugged in. This comes from the firmware, not from this app, and Windows does not
ask before trusting a new keyboard.

SYNCFASTERGB never opens interface 1, so you can disable it without affecting the lighting.
In an **administrator** PowerShell:

```powershell
# find the strip's keyboard (class Keyboard, "HID Keyboard Device")
Get-PnpDevice -Class Keyboard -PresentOnly | Where-Object InstanceId -like 'HID\VID_1A86&PID_FE07&MI_01*'
# disable it (use the InstanceId printed above; the USB\...\MI_01 interface node itself cannot be disabled)
Disable-PnpDevice -InstanceId 'HID\VID_1A86&PID_FE07&MI_01\<suffix>' -Confirm:$false
# alternative if the cmdlet refuses
pnputil /disable-device "HID\VID_1A86&PID_FE07&MI_01\<suffix>"
```

To undo, run `Enable-PnpDevice` (or `pnputil /enable-device`) with the same id. Other computers you plug the strip into are
still affected, and you should not plug it into a locked or unattended machine.

## Known limitations

- Releases are not Authenticode-signed, so Windows SmartScreen may warn on first run.
- A rogue USB device that imitates the strip's VID/PID can receive color data and send
  malformed replies; replies are bounds-checked, but treat unknown USB devices with care.
