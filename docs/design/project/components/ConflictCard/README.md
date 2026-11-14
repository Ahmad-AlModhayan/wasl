# ConflictCard

Both copies of a file that changed on both sides, with the three safe choices.

**Provide:** `name`, `kind`, `path`, `phone` and `drive` as `{ when, meta }`, `choice` (`phone` · `drive`) to highlight a picked side, `actions={false}` when the screen provides the choice itself.

- Keep both is the default and first. The copy you don't keep goes to the recycle bin, and the card says so.
- Show where each edit happened (*this phone*, *edited on the web*).

**Compose:** `OutlinedCard` with two `OutlinedCard` sides in a `Row` and three buttons in a `FlowRow`.
