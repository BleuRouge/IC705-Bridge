# Third-Party Notices — IC705-Bridge

IC705-Bridge incorporates or adapts software components developed by third parties.

These components remain subject to their respective licenses. The proprietary license of IC705-Bridge does not supersede or restrict the rights granted by those licenses.

## kappanhang

**Project:** kappanhang\
**Repository:** [github.com/nonoo/kappanhang](https://github.com/nonoo/kappanhang)\
**License:** MIT License

Portions of the RS-BA1 protocol implementation have been adapted from this project.

### Original License

[Original license file](https://github.com/nonoo/kappanhang/blob/master/LICENSE)

MIT License

Copyright (c) 2020 Norbert Varga HA2NON, Akos Marton ES1AKOS

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
/Icom is a registered trademark of Icom Incorporated (Japan)/

## Rust and JavaScript dependencies

The accompanying [THIRD_PARTY_DEPENDENCIES.txt](THIRD_PARTY_DEPENDENCIES.txt)
lists the versions, license expressions, copyright notices and license texts
for the Rust desktop dependency graph and the JavaScript production bundle.
The Rust inventory includes build dependencies for macOS (arm64 and x86_64),
Linux (x86_64) and Windows (x86_64). The JavaScript inventory includes code
embedded by build tooling and code vendored in npm packages.

### Mozilla Public License 2.0 source code

The corresponding Source Code Form of the MPL-covered Rust dependencies is
available under the MPL 2.0 at the links below. The archives contain the exact
versions used by this application, including their original source notices.

| Component | Source Code Form |
| --- | --- |
| `cssparser` 0.36.0 | [Source archive](https://static.crates.io/crates/cssparser/cssparser-0.36.0.crate) |
| `cssparser-macros` 0.6.1 | [Source archive](https://static.crates.io/crates/cssparser-macros/cssparser-macros-0.6.1.crate) |
| `dtoa-short` 0.3.5 | [Source archive](https://static.crates.io/crates/dtoa-short/dtoa-short-0.3.5.crate) |
| `option-ext` 0.2.0 | [Source archive](https://static.crates.io/crates/option-ext/option-ext-0.2.0.crate) |
| `selectors` 0.36.1 | [Source archive](https://static.crates.io/crates/selectors/selectors-0.36.1.crate) |

## Linux AppImage and LGPL permissions

The AppImage includes shared libraries licensed under the GNU Lesser General
Public License. `THIRD_PARTY_LINUX.txt` identifies them and provides their
copyright and license notices. The accompanying `linux-third-party-sources.tar`
in the same release provides corresponding third-party sources, including the
AppImage runtime and its build information.

For the combined work, Tao Gaudoneix permits recipients to modify it for their
own use and reverse engineer it to debug those modifications to the extent
required by the applicable LGPL. To replace a shared library, extract the
AppImage with `--appimage-extract`, replace the compatible library in
`squashfs-root/usr/lib` (or its subdirectory listed in the notices), and start
`squashfs-root/AppRun`. Third-party components retain their original terms.
