# Quickshell integration

`EmoticondService.qml` is a Quickshell singleton that runs `emoticond serve` and
speaks protocol v1 ([docs/protocol.md](../../docs/protocol.md)). It handles the
daemon's lifecycle, search, picks and the two report menus. It has no UI, so
you build the picker in your own shell's style. UI guidance is in
[docs/api-frontends.md §7](../../docs/api-frontends.md).

## Setup

1. Install `emoticond` and fetch its data (see the main README). The service
   runs `emoticond serve --frontend quickshell --idle 120`.
2. Copy `EmoticondService.qml` into a module directory of your shell, for
   example `services/`. The file name is the singleton's name.
3. If `emoticond` isn't on PATH, set the path:

   ```qml
   Component.onCompleted: EmoticondService.binary = "/path/to/emoticond"
   ```

   `repo` points the daemon at a development checkout instead
   (`EMOTICOND_REPO`); leave it empty for installed data.

## Use

```qml
// When the picker opens, start the engine so it is ready by the first key:
EmoticondService.ensureRunning()

// Send one search per keystroke. "" gives the browse list (starter faces, then history):
onTextChanged: EmoticondService.search(text)

// Re-send the query when the engine comes back after an idle exit:
Connections {
    target: EmoticondService
    function onReadyChanged() { if (EmoticondService.ready) EmoticondService.search(field.text) }
}

// Rows: kaomoji only, keyed by id. Use full-width rows and don't elide the face.
ListView {
    model: EmoticondService.results          // { id, text, kind, rank, from? }
    delegate: ... onClicked: { Quickshell.clipboardText = modelData.text; EmoticondService.pick(modelData.id, modelData.rank) }
}

// Reading line: EmoticondService.reading. Hide it when readingKind is "glyph" or "empty".
// Correction:   EmoticondService.corrected -> "Showing results for X"; searchUncorrected()
```

### Report menus

| Menu | Call | Items |
|---|---|---|
| query (on the reading line) | `reportQuery(reason)`, `noteQuery(text)` | `read_well`, `read_wrong` (exclusive), `missing` |
| face (one button per row) | `reportFace(id, rank, reason)`, `noteFace(id, rank, text)` | `great_fit`, `other_word`, `no_fit`, `offensive` |

- Each click sends one report right away. Choosing the current item again
  withdraws it. The current choices are in `readVerdict`, `missing`,
  `queryNoted` and `faceReports[id]`, and they reset when the query changes.
- `offensive` removes the row from `results` straight away, because the
  daemon blocklists the face everywhere. `hiddenFace` is then set: show
  *Hidden. Undo* and call `undoHide()`.
- Show `reportFooter` as a small line inside both menus. It is the shipped
  disclaimer text, never your own. `disclaimerText` is the full text.
- The daemon keeps one report per query and target, and a later report
  replaces an earlier one. So `read_well` followed by `missing` keeps only
  `missing` in the queue. Withdrawing one of the two sends the other again.
- Reports are kept in `~/.local/state/emoticond/` and sent in the
  background once they are 2 minutes old (docs/collector.md).

### Emoji and symbols

emoticond is kaomoji only. If your picker also offers emoji, keep your own
list and mix it in.

## Notes

- The idle exit (status 0) is not respawned. A crash after a good start is
  retried once. If the daemon never comes up, `failed` is set, also after
  `startTimeoutMs`, so you can fall back.
- `defaults` adds options to `set_defaults`, for example `({ safety: "moderate" })`.
  The user's `config.toml` `[profile.<frontend>]` still applies underneath.
- The browse list starts with the canonical faces (about 150 of them), and
  history comes after those. With a limit of 100, history doesn't show yet.
