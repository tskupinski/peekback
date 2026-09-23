# Source availability

Peekback's own code is licensed under Apache-2.0. The following unmodified
components retain their own licenses; those terms are not replaced by
Peekback's license.

## ELK / elkjs 0.9.3 (bundled by Mermaid)

This component's source is available under the Eclipse Public License 2.0.
The full license is included in `frontend/elkjs@0.9.3/LICENSE.md`.

- Published package: https://registry.npmjs.org/elkjs/-/elkjs-0.9.3.tgz
- Source and build instructions at the published package's git revision:
  https://github.com/kieler/elkjs/tree/a8304cf79fde75bc2ab1a89d28320f53f8637436
- ELK's upstream Java sources and releases: https://github.com/eclipse/elk

No upstream source modifications were made by Peekback. The npm integrity hash
and the Mermaid bundle hash are recorded in frontend/manifest.json.

## option-ext 0.2.0 (Rust dependency)

This component's source is available under Mozilla Public License 2.0. Release
archives include its unmodified published crate source in `sources/option-ext-0.2.0`
and its license in `rust/option-ext-0.2.0/LICENSE.txt`. It can also be obtained at:

- https://crates.io/api/v1/crates/option-ext/0.2.0/download
- https://github.com/soc/option-ext

## Other dual-licensed components

DOMPurify is distributed under its Apache-2.0 option. Individual upstream
notices and all collected alternative terms remain in the notice inventory.
`texts/` contains unmodified canonical SPDX texts for license identifiers and
exceptions referenced by dependencies. Template placeholders in those canonical
texts are not claims about authorship; package-specific notices are retained
alongside them.
