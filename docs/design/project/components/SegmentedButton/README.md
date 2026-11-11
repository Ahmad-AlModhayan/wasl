# SegmentedButton

Switches the scope of a picker between two or three places.

**Provide:** `items` (labels) and `value` (the selected one).

- Internal storage / SD card in the phone picker; My Drive / Shared with me in the Drive picker.
- Two or three options only.

**Compose:** `SingleChoiceSegmentedButtonRow { SegmentedButton(shape = SegmentedButtonDefaults.itemShape(i, n)) }`.
