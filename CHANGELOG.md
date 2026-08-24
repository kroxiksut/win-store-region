# Changelog

English only. The READMEs are translated; a changelog translated three times
would fall behind in two of them, and a stale changelog is worse than none.

What the program does is in the [README](README.md), and its limits are there
too. This file says only what changed between versions, so it stays short and
does not drift away from the document that describes the product.

Versions follow [semantic versioning](https://semver.org/). The leading zero is
not a hedge about quality: `1.0.0` promises that the interface is settled, and
that promise is not made until the program has run somewhere other than the
machine it was written on.

## 0.1.1 — 2026-08-24

Fixes only. Nothing moved in the window and nothing new was added.

- A record of an unfinished installation is no longer removed at startup. The
  application takes over an installation that is still running, and for one
  that has ended it writes the outcome into the history first — proving it from
  the package identity instead of calling it unknown.
- Failing to put the original region back is now reported instead of leaving
  the window restoring it forever, the attempt is repeated a few times, and a
  read-back that never answered after the write rolls the region back.
- An observation deadline now asks what to do instead of leaving an operation
  nobody can finish; an installation and an installer download are no longer
  blocked until the application is restarted.
- A pasted address of a Store subpage is no longer read as a product that does
  not exist, and a product the source has withdrawn is no longer reported as a
  temporary region that did not apply.

## 0.1.0 — 2026-08-23

First release. There is no previous version to compare it against.
