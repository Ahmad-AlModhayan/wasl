# TextField

Filled text field for names and patterns.

**Provide:** `label`, `value`, `helper` or `error`, `mono` for patterns and paths, `leading` / `trailing` icons.

- Pair names are required; the error says what to do (*Give this pair a name*).
- Skip patterns are comma-separated globs in `mono-medium`.

**Compose:** `TextField(label, supportingText, isError, textStyle = if (mono) MonoMedium else bodyLarge)`.
