# Security Policy

## Reporting a vulnerability

**A public issue is fine.** This is a small utility that changes one per-user
Windows setting, runs without elevation, stores nothing sensitive and talks to
nothing but Microsoft's own catalogue. There is no fleet of installations at
risk while a fix is written, so discussing a finding in the open costs little
and helps whoever reads the issue tracker next.

If you would rather report privately — because the finding involves someone
else's system, or you are simply more comfortable that way — use GitHub's
private vulnerability reporting on this repository (the "Report a vulnerability"
button under the Security tab). Either route is welcome; neither is treated as
the wrong one.

Please include:

- what an attacker can do, and what they need in order to do it;
- the steps that reproduce it, and the Windows build you saw it on;
- the application version, from **Help → About**;
- anything from `%LOCALAPPDATA%\WinStoreRegion\logs\winstoreregion.log` that is
  relevant. The log records file names and hashes but never paths, credentials,
  or telemetry — read it before attaching it, and remove anything you would
  rather not send.

You can expect an acknowledgement within a week. There is no bug bounty; this
is a single-maintainer project.

## Supported versions

Only the latest release is supported. The project is pre-1.0 and there are no
maintenance branches.

## What this application does, so you can judge what a finding means

Understanding the threat model is easier with the design in hand.

- **It changes one Windows setting.** The user's Home Location (`GeoId`) is
  switched for the duration of one operation and then restored. This is a
  per-user setting that Microsoft documents changing; it requires no elevation,
  and the application runs `asInvoker`. It is not a VPN, a proxy, an anonymity
  tool, or a Microsoft Account region changer, and it does not touch the network
  stack.
- **It never patches or bypasses anything.** Installation is performed by
  Microsoft Store and the WinGet COM API. Nothing is downloaded from unofficial
  servers, and no Microsoft binary or package is modified.
- **It writes only under `%LOCALAPPDATA%\WinStoreRegion\`:** the recovery record,
  the operation journal, and rotating diagnostic logs. It collects no telemetry
  and sends nothing anywhere.
- **Its only outbound network traffic** is a read-only query to Microsoft's Store
  catalogue asking whether a product is offered in a given market.

### The one place foreign code is executed

A Microsoft Store installer file selected by the user can be started, and this is
the only place in the product where a file the user supplied is executed. It is
gated deliberately narrowly, and findings against this path are the most
interesting ones:

- the file is inspected read-only, never loaded as a module;
- it is executed only with a **trusted Authenticode verdict** from
  `WinVerifyTrust` **and** a signer that passes the Microsoft Store signer
  policy;
- the user is shown the file name, the publisher and the SHA-256, and must
  confirm before anything is changed;
- immediately before the launch the file is verified again and its digest is
  compared with the one that was confirmed, so a file swapped in between gets no
  permission from that confirmation;
- the file is never modified, and it is started with no arguments;
- its exit code is never collected and never turned into a result.

A way to get an unsigned file, a file signed by someone else, or a file
different from the confirmed one to start through this path is a vulnerability.
So is any way to make the application change the region without publishing a
recovery record first, or to make it drop a record while the region is still
switched.

## Out of scope

- The consequences of using a Windows region other than where you live. That is
  a decision the user makes, and the application states it plainly.
- Microsoft Store's own behaviour, availability, and account or payment rules.
- Anything requiring an attacker who already has administrator rights or code
  execution as the user on the machine.
