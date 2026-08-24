# Third-Party Notices

TeleArk embeds the icon bundle from `gpui-component-assets 0.5.1`. That crate
declares Apache-2.0 and packages icons from the Lucide project. This notice is
included conservatively for those assets. It must remain with binary
distributions that embed the icon bundle.

Source inventory:

- `gpui-component-assets 0.5.1`: <https://github.com/longbridge/gpui-component/tree/v0.5.1/crates/assets>
- Lucide license and Feather-derived icon list: <https://github.com/lucide-icons/lucide/blob/main/LICENSE>

## Lucide Icons — ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

## Feather-derived Lucide icons — MIT License

Some Lucide icons are derived from the Feather project. TeleArk's embedded
bundle includes icons in that derived set, including common arrows, calendar,
check, chevrons, info, loader, lock, moon, plus, search, and upload.

Copyright (c) 2013-present Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Dependency-audit status

The locked graph has no packages with missing license declarations. Its
reviewed reciprocal declarations are limited to MPL-2.0 build/platform/helper
dependencies (`cbindgen`, `dwrote`, and `option-ext`) pulled by the published
GPUI baseline. Packages with alternative license expressions are consumed
under their permissive option. An exhaustive generated dependency notice and
final legal review are still required before a production release. The
repository's maintained `cargo-deny` policy provides the automated
dependency-license and advisory gate.
