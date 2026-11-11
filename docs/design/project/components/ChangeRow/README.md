# ChangeRow

One planned or completed action in a dry run or a running sync.

**Provide:** `action` (`upload` · `download` · `bin` · `conflict` · `skip` · `rename`), `kind` (`image` · `video` · `audio` · `pdf` · `doc` · `text`), `name`, `path`, `size`, `reason` (*New*, *Changed on Drive*, *Deleted on phone*).

- The action is written as a word as well as an arrow: *To Drive*, *To phone*, *To recycle bin*, *Conflict*, *Skipped*.
- Colours: to Drive `primary`, to phone `tertiary`, to bin `status-approval`, conflict `status-conflict`, skipped `on-surface-variant`.

**Compose:** custom `Row` at 72dp; file icon on `surfaceContainerHighest`, `shapes.small`.
