# Third-party notices: Wasl for desktop

The Wasl source is under PolyForm Shield 1.0.0 (see `LICENSE` at the repository root); the installed apps
carry no separate terms yet. Parts of the desktop build come from open-source projects under their own licences,
listed here. Keep this file with every copy of Wasl.

## Tauri NSIS installer template

`desktop/src-tauri/windows/installer.nsi` is derived from Tauri 2.12's NSIS template
(`crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`), and the installer is built with Tauri's
`utils.nsh`, `FileAssociation.nsh`, language files and `nsis_tauri_utils` plugin.

Copyright 2019-2024 Tauri Programme within The Commons Conservancy.
Licensed under either of the Apache License, Version 2.0 (https://www.apache.org/licenses/LICENSE-2.0)
or the MIT licence, at your option (SPDX: `Apache-2.0 OR MIT`). The MIT licence text:

> Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
> associated documentation files (the "Software"), to deal in the Software without restriction,
> including without limitation the rights to use, copy, modify, merge, publish, distribute,
> sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or
> substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
> NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
> NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES
> OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
> CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## NSIS

The installer is built with NSIS (Nullsoft Scriptable Install System), zlib/libpng licence,
https://nsis.sourceforge.io/License. Copyright (C) 1999-2025 Contributors.
