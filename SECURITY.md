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
| Live preview | The settings window reads the strip colors (at most 255 RGB values, not screen pixels) from a same-user shared-memory block; never written to disk or sent anywhere. |
| Strip power | Whole-strip power budget (default 40%, hard cap 80%); the app lowers it itself if the strip drops off the bus under load. |
| Settings | Stored in the per-user registry, not a file; size-capped, parsed strictly, every value range-clamped. The program creates no files. |
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

SYNCFASTERGB never opens interface 1, so it can be blocked without affecting the lighting.
Windows refuses to *disable* the keyboard device ("critical system device"), but a device-install
policy can stop its driver from loading. This edits a machine-wide Windows policy, so read it
first. In an **administrator** PowerShell:

```powershell
$k = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\DeviceInstall\Restrictions'
New-Item -Path "$k\DenyDeviceIDs" -Force | Out-Null
New-ItemProperty -Path $k -Name DenyDeviceIDs -Value 1 -PropertyType DWord -Force | Out-Null
New-ItemProperty -Path $k -Name DenyDeviceIDsRetroactive -Value 1 -PropertyType DWord -Force | Out-Null
New-ItemProperty -Path "$k\DenyDeviceIDs" -Name 1 -Value 'USB\VID_1A86&PID_FE07&MI_01' -PropertyType String -Force | Out-Null
```

Then unplug and replug the strip (or reboot). `MI_01` is the keyboard interface; the LED interface is
`MI_00` and is not affected. To undo (only if you had no other device-install policy):

```powershell
Remove-Item -Recurse 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\DeviceInstall\Restrictions'
```

Other computers you plug the strip into are still affected, and you should not plug it into a
locked or unattended machine.

## Known limitations

- Releases are not Authenticode-signed, so Windows SmartScreen may warn on first run.
- A rogue USB device that imitates the strip's VID/PID can receive color data and send
  malformed replies; replies are bounds-checked, but treat unknown USB devices with care.
