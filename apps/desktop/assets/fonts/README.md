# Fonts

Inter for every word that is read, titles and buttons included, and IBM Plex
Mono for the brand and for what is code: the session ID field, a failure's
code, a crash report. Both are built into the app (`src/theme/fonts.rs`) so
it looks the same on a machine that has neither installed, and both are
licensed under the SIL Open Font License 1.1: `Inter-OFL.txt` is the Inter
project's copy of it, `OFL.txt` IBM's.

**Why Inter**: it was drawn for interfaces on screens, with plain, even
shapes at the sizes a window uses (12 to 30 pixels), hinting that holds up
on Windows, and every letter Spanish needs. IBM Plex Sans, which it
replaces, reads as Plex Mono's sibling, and with titles and buttons in Plex
Mono the app looked like code.

Taken unchanged:

- Inter 4.1, `Inter-4.1.zip` from
  <https://github.com/rsms/inter/releases/tag/v4.1> (SHA-256
  `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`):
  `extras/ttf/` and `LICENSE.txt`, kept as `Inter-OFL.txt`.
- IBM Plex Mono from <https://github.com/IBM/plex> at commit
  `bc6a7e4b37634eb79785d59781893db63ee3edcb`, `packages/plex-mono`,
  `fonts/complete/ttf/`.

Only the weights the app draws with are here.

| File | SHA-256 |
|---|---|
| `Inter-Regular.ttf` | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| `Inter-Medium.ttf` | `97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872` |
| `Inter-SemiBold.ttf` | `78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3` |
| `Inter-Bold.ttf` | `288316099b1e0a47a4716d159098005eef7c0066921f34e3200393dbdb01947f` |
| `Inter-OFL.txt` | `262481e844521b326f5ecd053e59b98c8b2da78c8ee1bdbb6e8174305e54935a` |
| `IBMPlexMono-Regular.ttf` | `7c6fbddca4b700be918f5f6183d9bd4464fa427fe435f0b480d77fe2bb8c5a43` |
| `IBMPlexMono-BoldItalic.ttf` | `c8b00068fd2fd69bbcf62059148623f09d19e9ada8bf9e8caf1aedd01347046a` |
