# Changelog

English only. The READMEs are translated; a changelog translated eleven times
would fall behind in ten of them, and a stale changelog is worse than none.

What the program does is in the [README](README.md), and its limits are there
too. This file says only what changed between versions, so it stays short and
does not drift away from the document that describes the product.

Versions follow [semantic versioning](https://semver.org/). The leading zero is
not a hedge about quality: `1.0.0` promises that the interface is settled, and
that promise is not made until the program has run somewhere other than the
machine it was written on.

## 0.1.2 — 2026-09-03

Eight new interface languages, two of which made the window read right to left.

- Arabic, Persian, Traditional Chinese, Korean, Japanese, Turkish, Brazilian
  Portuguese and European Spanish, taking the interface from three languages to
  eleven. All eight are machine-made drafts that no native reader has checked,
  and every file says so at the top. The Traditional Chinese is written in
  Taiwan wording rather than converted character by character from the
  Simplified draft.
- Arabic and Persian read right to left, and so does the window while one of
  them is chosen. A language file says which way its language reads, and the
  whole layout follows: the badge and the command row start on the right,
  checkboxes put their box on the right of their caption, table columns run
  right to left, and scrollbars move to the left edge. Hebrew now needs that
  one field and a file, and no code at all.
- The drafts were audited here rather than taken on trust: one file did not
  parse, two put grammar on a placeholder whose value nobody knows until run
  time, two had a button caption too long for its button, and one addressed the
  user formally in half its sentences and informally in the other half. All of
  that is fixed; none of it was in the translation agent's own report.
- The language chooser is sorted by language tag instead of by the order the
  languages were added, and a language file no longer carries a position for
  someone to assign. A language added later takes its place among the others
  rather than at the end, and no other file has to be renumbered for it.

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
